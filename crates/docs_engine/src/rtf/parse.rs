//! RTF → a small document tree ([`Parsed`]): a tokenizer pass with a stack
//! of group states (character and paragraph formatting, destination, `\uc`).

use std::collections::{BTreeMap, BTreeSet};

use encoding_rs::Encoding;

use super::lex::{Lexer, Tok};

/// Deepest group nesting followed; deeper groups are skipped.
const MAX_DEPTH: usize = 400;
/// Largest picture kept (bytes).
const MAX_PICTURE: usize = 64 << 20;

// ── formatting ───────────────────────────────────────────────────────────────

/// Character formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CFmt {
    pub bold: bool,
    pub italic: bool,
    /// Word underline type (`single`, `double`, …), `None` for none.
    pub ul: Option<&'static str>,
    pub strike: bool,
    pub dstrike: bool,
    pub caps: bool,
    pub scaps: bool,
    pub hidden: bool,
    /// Half-points.
    pub size: i32,
    /// Font table index.
    pub font: Option<i32>,
    /// Colour table indexes; 0 is automatic.
    pub color: i32,
    pub highlight: i32,
    pub bg: i32,
    /// 1 superscript, -1 subscript.
    pub valign: i8,
    /// Character style (`\csN`).
    pub style: Option<i32>,
}

impl Default for CFmt {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            ul: None,
            strike: false,
            dstrike: false,
            caps: false,
            scaps: false,
            hidden: false,
            size: 24,
            font: None,
            color: 0,
            highlight: 0,
            bg: 0,
            valign: 0,
            style: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabStop {
    pub pos: i32,
    pub kind: &'static str,
    pub leader: Option<&'static str>,
}

/// An old-style (Word 95) list paragraph, `{\*\pn …}`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pn {
    /// `bullet`, `decimal`, `upperRoman`…
    pub nfc: &'static str,
    /// 0-based level (`\pnlvlN`), 0 for `\pnlvlblt` / `\pnlvlbody`.
    pub level: i32,
    pub start: i32,
    pub before: String,
    pub after: String,
    pub font: Option<i32>,
}

/// Paragraph formatting.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PFmt {
    pub style: i32,
    /// Word `w:jc` value; `None` for left.
    pub align: Option<&'static str>,
    pub li: i32,
    pub ri: i32,
    pub fi: i32,
    pub sb: i32,
    pub sa: i32,
    /// `\sl`: 0 single; with `slmult` 240ths of a line; else positive "at
    /// least", negative "exactly" (twips).
    pub sl: i32,
    pub slmult: bool,
    pub keepn: bool,
    pub keep: bool,
    pub pagebb: bool,
    pub contextual: bool,
    pub outline: Option<u8>,
    pub ls: Option<i32>,
    pub ilvl: i32,
    pub pn: Option<Pn>,
    pub intbl: bool,
    pub tabs: Vec<TabStop>,
}

// ── document tree ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Pic {
    pub png: bool,
    pub data: Vec<u8>,
    /// Size in EMU.
    pub cx: i64,
    pub cy: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String, CFmt),
    Tab(CFmt),
    Break(CFmt),
    PageBreak,
    Image(Pic),
    /// A field kept as a field (PAGE, NUMPAGES) with its shown result.
    Field {
        instr: String,
        shown: String,
        c: CFmt,
    },
    /// A hyperlink: `url`, or `#bookmark` for a link inside the document.
    Link {
        url: String,
        runs: Vec<Inline>,
    },
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Para {
    pub p: PFmt,
    pub inl: Vec<Inline>,
    /// Ends a section (`\sect`) with these settings.
    pub sect: Option<Box<SectFmt>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brdr {
    /// Word border type (`single`, `double`, …).
    pub kind: &'static str,
    /// Twips.
    pub width: i32,
    pub color: i32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellDef {
    pub right: i32,
    /// 1 first of a merge, 2 merged into the previous.
    pub hmerge: u8,
    pub vmerge: u8,
    /// Top, left, bottom, right.
    pub borders: [Option<Brdr>; 4],
    pub shade: i32,
    pub valign: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowDef {
    pub left: i32,
    pub gaph: i32,
    /// `\trpaddl` / `\trpaddr` (twips).
    pub pad: [Option<i32>; 2],
    /// Positive "at least", negative "exactly" (twips).
    pub height: i32,
    pub header: bool,
    pub jc: Option<&'static str>,
    /// Top, left, bottom, right, inside horizontal, inside vertical.
    pub borders: [Option<Brdr>; 6],
    pub cells: Vec<CellDef>,
}

impl Default for RowDef {
    fn default() -> Self {
        Self {
            left: 0,
            gaph: 108,
            pad: [None; 2],
            height: 0,
            header: false,
            jc: None,
            borders: [None; 6],
            cells: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub def: RowDef,
    pub cells: Vec<Vec<Para>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(Para),
    Table(Vec<Row>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HfType {
    Default,
    Even,
    First,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SectFmt {
    pub pgw: i32,
    pub pgh: i32,
    pub margins: [i32; 4],
    pub landscape: bool,
    pub header_y: i32,
    pub footer_y: i32,
    pub gutter: i32,
    pub titlepg: bool,
    /// Word `w:type` of the break that starts this section.
    pub brk: &'static str,
    pub pgn_start: Option<i32>,
    /// Headers (`.0 == false`) and footers defined in this section.
    pub hf: Vec<(bool, HfType, Vec<Block>)>,
}

impl Default for SectFmt {
    fn default() -> Self {
        Self {
            pgw: 12240,
            pgh: 15840,
            // Left, right, top, bottom.
            margins: [1800, 1800, 1440, 1440],
            landscape: false,
            header_y: 720,
            footer_y: 720,
            gutter: 0,
            titlepg: false,
            brk: "nextPage",
            pgn_start: None,
            hf: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    pub name: String,
    pub charset: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleKind {
    Para,
    Char,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub idx: i32,
    pub kind: StyleKind,
    pub name: String,
    pub c: CFmt,
    pub p: PFmt,
    pub next: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub start: i32,
    pub nfc: i32,
    pub jc: i32,
    pub follow: i32,
    /// `\leveltext` as read: a length character, then text with level
    /// placeholders as characters 0–8.
    pub text: String,
    pub li: i32,
    pub fi: i32,
    pub font: Option<i32>,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            start: 1,
            nfc: 0,
            jc: 0,
            follow: 0,
            text: String::new(),
            li: 0,
            fi: 0,
            font: None,
        }
    }
}

/// What the parser read.
pub struct Parsed {
    pub blocks: Vec<Block>,
    /// The last section's settings.
    pub sect: SectFmt,
    pub fonts: BTreeMap<i32, Font>,
    pub colors: Vec<Option<[u8; 3]>>,
    pub styles: Vec<Style>,
    pub lists: BTreeMap<i32, Vec<Level>>,
    /// `\ls` → `\listid`.
    pub overrides: BTreeMap<i32, i32>,
    /// Character formatting after `\plain`, paragraph after `\pard`.
    pub plain: CFmt,
    pub pard: PFmt,
    pub deftab: i32,
    pub facing: bool,
    /// What was left out, for the status line.
    pub notes: BTreeSet<&'static str>,
}

// ── parser ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dest {
    Body,
    /// Header or footer text.
    Hf(bool, HfType),
    /// Everything inside is ignored.
    Skip,
    /// Text is ignored, nested destinations are read.
    Ignore,
    FontTable,
    ColorTable,
    StyleSheet,
    ListTable,
    List,
    ListLevel,
    LevelText,
    ListOverrideTable,
    ListOverride,
    Pict,
    Field,
    FldInst,
    Pn,
    PnText(bool),
    DefChp,
    DefPap,
}

#[derive(Clone)]
struct State {
    c: CFmt,
    p: PFmt,
    dest: Dest,
    /// This group started `dest` (it ends with the group).
    fresh: bool,
    uc: u32,
}

#[derive(Default)]
struct Sink {
    blocks: Vec<Block>,
    inl: Vec<Inline>,
    rows: Vec<Row>,
    cells: Vec<Vec<Para>>,
    cell: Vec<Para>,
    /// Counts finished paragraphs (fields must end in the one they start).
    serial: u64,
    /// Text isn't merged into inlines before this index (a field result
    /// starts here).
    fence: usize,
}

struct FieldCtx {
    instr: String,
    /// Sink depth, inline index and paragraph serial where the result starts.
    result: Option<(usize, usize, u64)>,
}

#[derive(Default)]
struct PictBuf {
    kind: u8,
    data: Vec<u8>,
    nibble: Option<u8>,
    picw: i32,
    pich: i32,
    wgoal: i32,
    hgoal: i32,
    sx: i32,
    sy: i32,
}

#[derive(Clone, Copy)]
enum BorderTarget {
    None,
    Cell(usize),
    Row(usize),
}

struct Parser {
    stack: Vec<State>,
    pending: Vec<u8>,
    skip: u32,
    high: Option<u16>,
    star: bool,
    ansi: &'static Encoding,
    fonts: BTreeMap<i32, Font>,
    font_id: Option<i32>,
    font_charset: i32,
    font_name: String,
    colors: Vec<Option<[u8; 3]>>,
    rgb: [Option<u8>; 3],
    styles: Vec<Style>,
    style_name: String,
    style_idx: i32,
    style_kind: StyleKind,
    style_next: Option<i32>,
    lists: BTreeMap<i32, Vec<Level>>,
    list_id: i32,
    list_levels: Vec<Level>,
    level: Level,
    level_text: String,
    overrides: BTreeMap<i32, i32>,
    ov: (i32, i32),
    pn: Pn,
    pn_text: String,
    fields: Vec<FieldCtx>,
    pict: PictBuf,
    sinks: Vec<Sink>,
    row: RowDef,
    cell: CellDef,
    border: BorderTarget,
    tab_kind: &'static str,
    tab_leader: Option<&'static str>,
    doc_sect: SectFmt,
    sect: SectFmt,
    plain: CFmt,
    pard: PFmt,
    deftab: i32,
    facing: bool,
    notes: BTreeSet<&'static str>,
    /// The document's closing brace was read.
    done: bool,
    /// Paragraph formatting when the document ended (for text after the
    /// last `\par`).
    end_p: Option<PFmt>,
}

/// The encoding for a font's `\fcharset`; `None` for "use the document's".
fn charset_encoding(charset: i32) -> Option<&'static Encoding> {
    use encoding_rs::*;
    Some(match charset {
        77 => MACINTOSH,
        128 => SHIFT_JIS,
        129 | 130 => EUC_KR,
        134 => GBK,
        136 => BIG5,
        161 => WINDOWS_1253,
        162 => WINDOWS_1254,
        163 => WINDOWS_1258,
        177 => WINDOWS_1255,
        178 => WINDOWS_1256,
        186 => WINDOWS_1257,
        204 => WINDOWS_1251,
        222 => WINDOWS_874,
        238 => WINDOWS_1250,
        _ => return None,
    })
}

/// The encoding for `\ansicpgN`.
fn codepage_encoding(cp: i32) -> Option<&'static Encoding> {
    use encoding_rs::*;
    Some(match cp {
        874 => WINDOWS_874,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1252 => WINDOWS_1252,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        866 => IBM866,
        10000 => MACINTOSH,
        20866 => KOI8_R,
        65001 => UTF_8,
        _ => return None,
    })
}

/// Destinations that `\*` may introduce and that are read.
fn star_known(w: &str) -> bool {
    matches!(
        w,
        "listtable"
            | "listoverridetable"
            | "fldinst"
            | "pn"
            | "pntxta"
            | "pntxtb"
            | "shppict"
            | "shpinst"
            | "defchp"
            | "defpap"
            | "cs"
            | "ds"
            | "ts"
            | "pict"
            | "header"
            | "footer"
            | "headerl"
            | "headerr"
            | "headerf"
            | "footerl"
            | "footerr"
            | "footerf"
    )
}

impl Parser {
    fn new() -> Self {
        let state = State {
            c: CFmt::default(),
            p: PFmt::default(),
            dest: Dest::Body,
            fresh: false,
            uc: 1,
        };
        Self {
            stack: vec![state],
            pending: Vec::new(),
            skip: 0,
            high: None,
            star: false,
            ansi: encoding_rs::WINDOWS_1252,
            fonts: BTreeMap::new(),
            font_id: None,
            font_charset: 0,
            font_name: String::new(),
            colors: Vec::new(),
            rgb: [None; 3],
            styles: Vec::new(),
            style_name: String::new(),
            style_idx: 0,
            style_kind: StyleKind::Para,
            style_next: None,
            lists: BTreeMap::new(),
            list_id: 0,
            list_levels: Vec::new(),
            level: Level::default(),
            level_text: String::new(),
            overrides: BTreeMap::new(),
            ov: (0, 0),
            pn: Pn::default(),
            pn_text: String::new(),
            fields: Vec::new(),
            pict: PictBuf::default(),
            sinks: vec![Sink::default()],
            row: RowDef::default(),
            cell: CellDef::default(),
            border: BorderTarget::None,
            tab_kind: "left",
            tab_leader: None,
            doc_sect: SectFmt::default(),
            sect: SectFmt::default(),
            plain: CFmt::default(),
            pard: PFmt::default(),
            deftab: 720,
            facing: false,
            notes: BTreeSet::new(),
            done: false,
            end_p: None,
        }
    }

    fn top(&mut self) -> &mut State {
        self.stack.last_mut().expect("the root state is never popped")
    }

    fn dest(&self) -> Dest {
        self.stack.last().map_or(Dest::Skip, |s| s.dest)
    }

    fn sink(&mut self) -> &mut Sink {
        self.sinks.last_mut().expect("the body sink is never popped")
    }

    fn encoding(&self) -> &'static Encoding {
        let charset = match self.dest() {
            Dest::FontTable => Some(self.font_charset),
            _ => self
                .stack
                .last()
                .and_then(|s| s.c.font)
                .and_then(|f| self.fonts.get(&f))
                .map(|f| f.charset),
        };
        charset.and_then(charset_encoding).unwrap_or(self.ansi)
    }

    fn run(&mut self, bytes: &[u8]) {
        for tok in Lexer::new(bytes) {
            if self.done {
                break;
            }
            match tok {
                Tok::Text(t) => self.bytes(t),
                Tok::Hex(b) => self.hex(b),
                other => {
                    self.flush();
                    match other {
                        Tok::Open => self.open(),
                        Tok::Close => self.close(),
                        Tok::Word(w, p) => self.word(w, p),
                        Tok::Symbol(c) => self.symbol(c),
                        Tok::Bin(data) => self.bin(data),
                        Tok::Text(_) | Tok::Hex(_) => {}
                    }
                }
            }
        }
        self.flush();
    }

    fn bytes(&mut self, t: &[u8]) {
        let dest = self.dest();
        if dest == Dest::Skip {
            return;
        }
        for &b in t {
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            if dest == Dest::Pict {
                self.pict_hex(b);
            } else {
                self.pending.push(b);
            }
        }
    }

    fn hex(&mut self, b: u8) {
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        if !matches!(self.dest(), Dest::Skip | Dest::Pict) {
            self.pending.push(b);
        }
    }

    fn bin(&mut self, data: &[u8]) {
        self.skip = 0;
        if self.dest() == Dest::Pict && self.pict.data.len() + data.len() <= MAX_PICTURE {
            self.pict.data.extend_from_slice(data);
        }
    }

    fn pict_hex(&mut self, b: u8) {
        let v = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return,
        };
        match self.pict.nibble.take() {
            Some(h) => {
                if self.pict.data.len() < MAX_PICTURE {
                    self.pict.data.push(h << 4 | v);
                }
            }
            None => self.pict.nibble = Some(v),
        }
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.pending);
        let (s, _) = self.encoding().decode_without_bom_handling(&bytes);
        let s = s.into_owned();
        self.high = None;
        self.text(&s);
    }

    fn open(&mut self) {
        self.skip = 0;
        self.star = false;
        let mut st = self.stack.last().cloned().expect("root state");
        st.fresh = false;
        if self.stack.len() >= MAX_DEPTH {
            st.dest = Dest::Skip;
        }
        match st.dest {
            // A style or list level starts from the defaults.
            Dest::StyleSheet | Dest::List => {
                st.c = self.plain.clone();
                st.p = self.pard.clone();
                if st.dest == Dest::StyleSheet {
                    self.style_idx = 0;
                    self.style_kind = StyleKind::Para;
                    self.style_next = None;
                    self.style_name.clear();
                }
            }
            _ => {}
        }
        self.stack.push(st);
    }

    fn close(&mut self) {
        self.skip = 0;
        self.star = false;
        self.high = None;
        if self.stack.len() <= 1 {
            // A `}` before any `{`.
            self.done = true;
            return;
        }
        let st = self.stack.pop().expect("checked above");
        if self.stack.len() == 1 {
            // The document's closing brace: anything after it is ignored.
            self.done = true;
            self.end_p = Some(st.p.clone());
        }
        if st.dest == Dest::StyleSheet && !st.fresh && !self.style_name.trim().is_empty() {
            // An entry without its `;`.
            self.end_style(&st);
        }
        if st.fresh {
            self.finish(st);
        }
    }

    /// The group that started a destination ended.
    fn finish(&mut self, st: State) {
        match st.dest {
            Dest::FontTable => self.end_font(),
            Dest::ListLevel => {
                let mut level = std::mem::take(&mut self.level);
                level.li = st.p.li;
                level.fi = st.p.fi;
                level.font = st.c.font;
                if self.list_levels.len() < 9 {
                    self.list_levels.push(level);
                }
            }
            Dest::List => {
                let levels = std::mem::take(&mut self.list_levels);
                self.lists.insert(self.list_id, levels);
            }
            Dest::LevelText => self.level.text = std::mem::take(&mut self.level_text),
            Dest::ListOverride => {
                let (list, ls) = self.ov;
                self.overrides.insert(ls, list);
            }
            Dest::Pn => {
                let pn = std::mem::take(&mut self.pn);
                self.top().p.pn = Some(pn);
            }
            Dest::PnText(after) => {
                let t = std::mem::take(&mut self.pn_text);
                if after {
                    self.pn.after = t;
                } else {
                    self.pn.before = t;
                }
            }
            Dest::DefChp => {
                let font = self.plain.font;
                self.plain = st.c;
                self.plain.font = self.plain.font.or(font);
                for s in &mut self.stack {
                    s.c = self.plain.clone();
                }
            }
            Dest::DefPap => {
                self.pard = st.p;
                for s in &mut self.stack {
                    s.p = self.pard.clone();
                }
            }
            Dest::Pict => self.end_pict(),
            Dest::Field => self.end_field(),
            Dest::Hf(footer, kind) => {
                if self.sinks.len() > 1 {
                    let mut sink = self.sinks.pop().expect("checked");
                    let blocks = Self::close_sink(&mut sink, st.p.clone(), &self.row);
                    self.sect.hf.retain(|(f, k, _)| (*f, *k) != (footer, kind));
                    self.sect.hf.push((footer, kind, blocks));
                }
            }
            _ => {}
        }
    }

    /// The body or header/footer the current group is in.
    fn body_dest(&self) -> Dest {
        self.stack
            .iter()
            .rev()
            .find_map(|s| match s.dest {
                d @ (Dest::Body | Dest::Hf(..)) => Some(d),
                _ => None,
            })
            .unwrap_or(Dest::Body)
    }

    fn start(&mut self, dest: Dest) {
        let st = self.top();
        st.dest = dest;
        st.fresh = true;
    }

    fn symbol(&mut self, c: u8) {
        if c == b'*' {
            self.star = true;
            return;
        }
        self.star = false;
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        let ch = match c {
            b'\\' | b'{' | b'}' => c as char,
            b'~' => '\u{a0}',
            b'_' => '\u{2011}',
            b'-' => '\u{ad}',
            b'\t' => '\t',
            _ => return,
        };
        self.text(ch.encode_utf8(&mut [0; 4]));
    }

    /// Decoded text, routed by destination.
    fn text(&mut self, s: &str) {
        match self.dest() {
            Dest::Body | Dest::Hf(..) => self.body_text(s),
            Dest::FontTable => {
                for ch in s.chars() {
                    if ch == ';' {
                        self.end_font();
                    } else {
                        self.font_name.push(ch);
                    }
                }
            }
            Dest::ColorTable => {
                for _ in s.matches(';') {
                    let rgb = match self.rgb {
                        [None, None, None] => None,
                        [r, g, b] => Some([r.unwrap_or(0), g.unwrap_or(0), b.unwrap_or(0)]),
                    };
                    self.colors.push(rgb);
                    self.rgb = [None; 3];
                }
            }
            Dest::StyleSheet => {
                for ch in s.chars() {
                    if ch == ';' {
                        let st = self.stack.last().cloned().expect("root");
                        self.end_style(&st);
                    } else {
                        self.style_name.push(ch);
                    }
                }
            }
            Dest::LevelText => self.level_text.push_str(s),
            Dest::PnText(_) => self.pn_text.push_str(s),
            Dest::FldInst => {
                if let Some(f) = self.fields.last_mut() {
                    f.instr.push_str(s);
                }
            }
            _ => {}
        }
    }

    fn body_text(&mut self, s: &str) {
        let c = self.stack.last().map(|s| s.c.clone()).unwrap_or_default();
        if c.hidden {
            return;
        }
        let mut buf = String::new();
        for ch in s.chars() {
            match ch {
                '\t' => {
                    self.push_text(&mut buf, &c);
                    self.sink().inl.push(Inline::Tab(c.clone()));
                }
                c if (c as u32) < 0x20 || c == '\u{7f}' => {}
                ch => buf.push(ch),
            }
        }
        self.push_text(&mut buf, &c);
    }

    fn push_text(&mut self, buf: &mut String, c: &CFmt) {
        if buf.is_empty() {
            return;
        }
        let sink = self.sink();
        let mergeable = sink.inl.len() > sink.fence;
        let inl = &mut sink.inl;
        if mergeable
            && let Some(Inline::Text(t, last)) = inl.last_mut()
            && last == c
        {
            t.push_str(buf);
        } else {
            inl.push(Inline::Text(buf.clone(), c.clone()));
        }
        buf.clear();
    }

    fn end_font(&mut self) {
        let name = self.font_name.trim().to_owned();
        if let Some(id) = self.font_id.take()
            && !name.is_empty()
        {
            self.fonts.insert(
                id,
                Font {
                    name,
                    charset: self.font_charset,
                },
            );
        }
        self.font_name.clear();
        self.font_charset = 0;
    }

    fn end_style(&mut self, st: &State) {
        let name = self.style_name.trim().to_owned();
        self.style_name.clear();
        if name.is_empty() || self.style_kind == StyleKind::Other {
            return;
        }
        let idx = self.style_idx;
        self.styles.retain(|s| !(s.idx == idx && s.kind == self.style_kind));
        let mut c = st.c.clone();
        c.style = None;
        let mut p = st.p.clone();
        p.style = idx;
        p.intbl = false;
        p.ls = None;
        p.pn = None;
        self.styles.push(Style {
            idx,
            kind: self.style_kind,
            name,
            c,
            p,
            next: self.style_next,
        });
    }

    fn end_pict(&mut self) {
        let pict = std::mem::take(&mut self.pict);
        if pict.kind == 0 || pict.kind > 2 {
            self.notes
                .insert("pictures in formats other than PNG and JPEG were left out");
            return;
        }
        if pict.data.is_empty() {
            return;
        }
        let scale = |v: i32, s: i32| i64::from(v) * i64::from(if s > 0 { s } else { 100 }) / 100;
        // Twips: the goal size, else the picture's own size in pixels (96 dpi).
        let w = if pict.wgoal > 0 {
            pict.wgoal
        } else {
            pict.picw.saturating_mul(15)
        };
        let h = if pict.hgoal > 0 {
            pict.hgoal
        } else {
            pict.pich.saturating_mul(15)
        };
        let clamp = |t: i64| t.clamp(15, 31_680) * 635;
        let (cx, cy) = if w > 0 && h > 0 {
            (clamp(scale(w, pict.sx)), clamp(scale(h, pict.sy)))
        } else {
            (1440 * 635, 1440 * 635)
        };
        let pic = Pic {
            png: pict.kind == 1,
            data: pict.data,
            cx,
            cy,
        };
        self.sink().inl.push(Inline::Image(pic));
    }

    fn end_field(&mut self) {
        let Some(f) = self.fields.pop() else { return };
        let instr = f.instr.trim().to_owned();
        let words = field_words(&instr);
        let Some(first) = words.first() else { return };
        let first = first.to_ascii_uppercase();
        let here = (self.sinks.len() - 1, self.sinks.last().map_or(0, |s| s.serial));
        let start = f
            .result
            .filter(|&(sink, at, serial)| {
                (sink, serial) == here && at <= self.sinks[sink].inl.len()
            })
            .map(|(_, at, _)| at);
        match first.as_str() {
            "HYPERLINK" => {
                let Some(start) = start else { return };
                let mut url = None;
                let mut anchor = None;
                let mut it = words.iter().skip(1);
                while let Some(w) = it.next() {
                    match w.as_str() {
                        "\\l" => anchor = it.next().cloned(),
                        "\\o" | "\\t" => {
                            it.next();
                        }
                        s if s.starts_with('\\') => {}
                        s => {
                            if url.is_none() {
                                url = Some(s.to_owned());
                            }
                        }
                    }
                }
                let url = match (url, anchor) {
                    (Some(u), Some(a)) => format!("{u}#{a}"),
                    (Some(u), None) => u,
                    (None, Some(a)) => format!("#{a}"),
                    (None, None) => return,
                };
                let inl = &mut self.sinks[here.0].inl;
                let mut runs: Vec<Inline> = inl.split_off(start);
                if runs.is_empty() {
                    return;
                }
                // Links don't nest.
                runs = runs
                    .into_iter()
                    .flat_map(|r| match r {
                        Inline::Link { runs, .. } => runs,
                        r => vec![r],
                    })
                    .collect();
                inl.push(Inline::Link { url, runs });
            }
            "PAGE" | "NUMPAGES" => {
                let c = self.stack.last().map(|s| s.c.clone()).unwrap_or_default();
                let inl = &mut self.sinks[here.0].inl;
                let (shown, c) = match start {
                    Some(start) => {
                        let runs = inl.split_off(start);
                        let c = runs
                            .iter()
                            .find_map(|r| match r {
                                Inline::Text(_, c) => Some(c.clone()),
                                _ => None,
                            })
                            .unwrap_or(c);
                        let shown: String = runs
                            .iter()
                            .filter_map(|r| match r {
                                Inline::Text(t, _) => Some(t.as_str()),
                                _ => None,
                            })
                            .collect();
                        (shown, c)
                    }
                    None => ("1".to_owned(), c),
                };
                let shown = if shown.trim().is_empty() {
                    "1".to_owned()
                } else {
                    shown
                };
                inl.push(Inline::Field {
                    instr: first,
                    shown,
                    c,
                });
            }
            _ => {}
        }
    }

    /// Finish the open paragraph of `sink` and any table; its blocks.
    fn close_sink(sink: &mut Sink, p: PFmt, row: &RowDef) -> Vec<Block> {
        if !sink.inl.is_empty() {
            let para = Para {
                p,
                inl: std::mem::take(&mut sink.inl),
                sect: None,
            };
            if para.p.intbl {
                sink.cell.push(para);
            } else {
                Self::flush_table(sink, row);
                sink.blocks.push(Block::Para(para));
            }
        }
        Self::flush_table(sink, row);
        std::mem::take(&mut sink.blocks)
    }

    /// Turn collected rows (and a row left open) into a table block.
    fn flush_table(sink: &mut Sink, def: &RowDef) {
        if !sink.cell.is_empty() {
            let cell = std::mem::take(&mut sink.cell);
            sink.cells.push(cell);
        }
        if !sink.cells.is_empty() {
            let cells = std::mem::take(&mut sink.cells);
            sink.rows.push(Row {
                def: def.clone(),
                cells,
            });
        }
        if !sink.rows.is_empty() {
            let rows = std::mem::take(&mut sink.rows);
            sink.blocks.push(Block::Table(rows));
        }
    }

    /// `\par`, `\cell`, `\sect`: end the paragraph.
    fn end_para(&mut self, cell: bool, sect: Option<Box<SectFmt>>) {
        let mut p = self.stack.last().map(|s| s.p.clone()).unwrap_or_default();
        if cell {
            p.intbl = true;
        }
        let row = self.row.clone();
        let sink = self.sink();
        sink.serial += 1;
        sink.fence = 0;
        let para = Para {
            p,
            inl: std::mem::take(&mut sink.inl),
            sect,
        };
        if para.p.intbl {
            sink.cell.push(para);
        } else {
            Self::flush_table(sink, &row);
            sink.blocks.push(Block::Para(para));
        }
    }

    fn end_cell(&mut self) {
        self.end_para(true, None);
        let sink = self.sink();
        let cell = std::mem::take(&mut sink.cell);
        sink.cells.push(cell);
    }

    fn end_row(&mut self) {
        let row = self.row.clone();
        let sink = self.sink();
        if !sink.inl.is_empty() || !sink.cell.is_empty() {
            let p = PFmt {
                intbl: true,
                ..PFmt::default()
            };
            let para = Para {
                p,
                inl: std::mem::take(&mut sink.inl),
                sect: None,
            };
            sink.cell.push(para);
            let cell = std::mem::take(&mut sink.cell);
            sink.cells.push(cell);
        }
        let cells = std::mem::take(&mut sink.cells);
        sink.rows.push(Row { def: row, cells });
    }

    fn border(&mut self) -> Option<&mut Brdr> {
        let slot = match self.border {
            BorderTarget::None => return None,
            BorderTarget::Cell(i) => &mut self.cell.borders[i],
            BorderTarget::Row(i) => &mut self.row.borders[i],
        };
        Some(slot.get_or_insert(Brdr {
            kind: "single",
            width: 10,
            color: 0,
        }))
    }

    fn set_border_kind(&mut self, kind: &'static str) {
        if let Some(b) = self.border() {
            b.kind = kind;
        }
    }

    fn word(&mut self, w: &str, param: Option<i32>) {
        let star = std::mem::take(&mut self.star);
        let dest = self.dest();
        if dest == Dest::Skip {
            return;
        }
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        // (LibreOffice writes `\uc0 \u-10179\uc1 \u-8704`.)
        if !matches!(w, "u" | "uc") {
            self.high = None;
        }
        let n = param.unwrap_or(0);
        let on = param != Some(0);
        // Destinations.
        match w {
            "fonttbl" => return self.start(Dest::FontTable),
            "colortbl" => return self.start(Dest::ColorTable),
            "stylesheet" => return self.start(Dest::StyleSheet),
            "listtable" => return self.start(Dest::ListTable),
            "listoverridetable" => return self.start(Dest::ListOverrideTable),
            "list" if dest == Dest::ListTable => {
                self.list_id = 0;
                self.list_levels.clear();
                return self.start(Dest::List);
            }
            "listlevel" if dest == Dest::List => {
                self.level = Level::default();
                return self.start(Dest::ListLevel);
            }
            "leveltext" if dest == Dest::ListLevel => {
                self.level_text.clear();
                return self.start(Dest::LevelText);
            }
            "listoverride" if dest == Dest::ListOverrideTable => {
                self.ov = (0, 0);
                return self.start(Dest::ListOverride);
            }
            "pict" => {
                self.pict = PictBuf::default();
                return self.start(Dest::Pict);
            }
            "field" => {
                self.fields.push(FieldCtx {
                    instr: String::new(),
                    result: None,
                });
                return self.start(Dest::Field);
            }
            "fldinst" if dest == Dest::Field => return self.start(Dest::FldInst),
            "fldrslt" if dest == Dest::Field => {
                let sink = self.sinks.len() - 1;
                let (at, serial) = (self.sinks[sink].inl.len(), self.sinks[sink].serial);
                self.sinks[sink].fence = at;
                if let Some(f) = self.fields.last_mut() {
                    f.result = Some((sink, at, serial));
                }
                self.top().dest = self.body_dest();
                return;
            }
            "pn" => {
                self.pn = Pn {
                    nfc: "decimal",
                    start: 1,
                    ..Pn::default()
                };
                return self.start(Dest::Pn);
            }
            "pntxta" if dest == Dest::Pn => {
                self.pn_text.clear();
                return self.start(Dest::PnText(true));
            }
            "pntxtb" if dest == Dest::Pn => {
                self.pn_text.clear();
                return self.start(Dest::PnText(false));
            }
            "defchp" => return self.start(Dest::DefChp),
            "defpap" => return self.start(Dest::DefPap),
            "header" | "headerr" | "headerl" | "headerf" | "footer" | "footerr" | "footerl"
            | "footerf" => {
                if self.sinks.len() > 1 || !matches!(dest, Dest::Body) {
                    return self.start(Dest::Skip);
                }
                let kind = match w.as_bytes().last() {
                    Some(b'l') => HfType::Even,
                    Some(b'f') => HfType::First,
                    _ => HfType::Default,
                };
                self.sinks.push(Sink::default());
                let pard = self.pard.clone();
                let plain = self.plain.clone();
                let st = self.top();
                st.p = pard;
                st.c = plain;
                return self.start(Dest::Hf(w.starts_with("footer"), kind));
            }
            // Read through: their content is ordinary text or a picture.
            "shppict" | "shp" | "ud" => return,
            "result" => {
                self.top().dest = self.body_dest();
                return;
            }
            "shpinst" | "object" => {
                self.top().dest = Dest::Ignore;
                return;
            }
            "footnote" => {
                self.notes.insert("footnotes were left out");
                return self.start(Dest::Skip);
            }
            "annotation" => {
                self.notes.insert("comments were left out");
                return self.start(Dest::Skip);
            }
            "shptxt" | "do" => {
                self.notes.insert("text boxes and drawings were left out");
                return self.start(Dest::Skip);
            }
            "info" | "pntext" | "listtext" | "nonshppict" | "filetbl" | "revtbl" | "xe"
            | "tc" | "tcn" | "txe" | "rxe" | "listname" | "levelnumbers" | "lfolevel"
            | "shprslt" | "upr" | "fontemb" | "fontfile" | "atnid" | "atnauthor" | "pgdsctbl"
            | "objdata" | "objclass" | "bkmkstart" | "bkmkend" | "datafield" | "ffdata"
            | "picprop" | "blipuid" | "template" | "userprops" | "docvar" | "comment" => {
                return self.start(Dest::Skip);
            }
            _ => {}
        }
        if star && !star_known(w) {
            return self.start(Dest::Skip);
        }
        match dest {
            Dest::FontTable => {
                match w {
                    "f" => {
                        self.end_font();
                        self.font_id = Some(n);
                    }
                    "fcharset" => self.font_charset = n,
                    _ => {}
                }
                return;
            }
            Dest::ColorTable => {
                let v = Some(n.clamp(0, 255) as u8);
                match w {
                    "red" => self.rgb[0] = v,
                    "green" => self.rgb[1] = v,
                    "blue" => self.rgb[2] = v,
                    _ => {}
                }
                return;
            }
            Dest::StyleSheet => match w {
                "s" => {
                    self.style_idx = n;
                    self.style_kind = StyleKind::Para;
                    return;
                }
                "cs" => {
                    self.style_idx = n;
                    self.style_kind = StyleKind::Char;
                    return;
                }
                "ds" | "ts" => {
                    self.style_idx = n;
                    self.style_kind = StyleKind::Other;
                    return;
                }
                "snext" => {
                    self.style_next = Some(n);
                    return;
                }
                "sbasedon" | "additive" | "sautoupd" | "shidden" | "sqformat" | "spriority"
                | "ssemihidden" | "sunhideused" | "slink" | "slocked" => return,
                _ => {}
            },
            Dest::List => {
                if w == "listid" {
                    self.list_id = n;
                }
                return;
            }
            Dest::ListOverride => {
                match w {
                    "listid" => self.ov.0 = n,
                    "ls" => self.ov.1 = n,
                    _ => {}
                }
                return;
            }
            Dest::ListLevel => match w {
                "levelnfc" | "levelnfcn" => {
                    self.level.nfc = n;
                    return;
                }
                "leveljc" | "leveljcn" => {
                    self.level.jc = n;
                    return;
                }
                "levelstartat" => {
                    self.level.start = n;
                    return;
                }
                "levelfollow" => {
                    self.level.follow = n;
                    return;
                }
                _ => {}
            },
            Dest::Pict => {
                match w {
                    "pngblip" => self.pict.kind = 1,
                    "jpegblip" => self.pict.kind = 2,
                    "emfblip" | "wmetafile" | "macpict" | "dibitmap" | "wbitmap"
                    | "pmmetafile" => self.pict.kind = 3,
                    "picw" => self.pict.picw = n,
                    "pich" => self.pict.pich = n,
                    "picwgoal" => self.pict.wgoal = n,
                    "pichgoal" => self.pict.hgoal = n,
                    "picscalex" => self.pict.sx = n,
                    "picscaley" => self.pict.sy = n,
                    _ => {}
                }
                return;
            }
            Dest::Pn => {
                let nfc = match w {
                    "pnlvlblt" => {
                        self.pn.nfc = "bullet";
                        self.pn.level = 0;
                        return;
                    }
                    "pnlvlbody" => {
                        self.pn.level = 0;
                        return;
                    }
                    "pnlvl" => {
                        self.pn.level = (n - 1).clamp(0, 8);
                        return;
                    }
                    "pnstart" => {
                        self.pn.start = n;
                        return;
                    }
                    "pnf" => {
                        self.pn.font = Some(n);
                        return;
                    }
                    "pndec" => "decimal",
                    "pnucrm" => "upperRoman",
                    "pnlcrm" => "lowerRoman",
                    "pnucltr" => "upperLetter",
                    "pnlcltr" => "lowerLetter",
                    "pnord" => "ordinal",
                    _ => return,
                };
                if self.pn.nfc != "bullet" {
                    self.pn.nfc = nfc;
                }
                return;
            }
            _ => {}
        }
        self.control(w, param, n, on);
    }

    /// Formatting and content control words.
    fn control(&mut self, w: &str, param: Option<i32>, n: i32, on: bool) {
        let body = matches!(self.dest(), Dest::Body | Dest::Hf(..));
        // Word repeats `\sectd…` inside headers and footers; they belong to
        // the section already.
        let section_word = matches!(
            w,
            "sectd"
                | "pgwsxn"
                | "pghsxn"
                | "marglsxn"
                | "margrsxn"
                | "margtsxn"
                | "margbsxn"
                | "guttersxn"
                | "lndscpsxn"
                | "titlepg"
                | "headery"
                | "footery"
                | "sbknone"
                | "sbkcol"
                | "sbkpage"
                | "sbkeven"
                | "sbkodd"
                | "pgnstarts"
        );
        if section_word && self.sinks.len() > 1 {
            return;
        }
        match w {
            // ── document ──
            "ansicpg" => {
                if let Some(e) = codepage_encoding(n) {
                    self.ansi = e;
                }
            }
            "mac" => self.ansi = encoding_rs::MACINTOSH,
            "deff" => {
                self.plain.font = Some(n);
                for s in &mut self.stack {
                    s.c.font = s.c.font.or(Some(n));
                }
            }
            "deftab" => self.deftab = n.clamp(1, 31_680),
            "facingp" => self.facing = true,
            "paperw" | "paperh" | "margl" | "margr" | "margt" | "margb" | "gutter" => {
                let v = n.clamp(0, 31_680 * 2);
                for s in [&mut self.doc_sect, &mut self.sect] {
                    match w {
                        "paperw" => s.pgw = v.max(1440),
                        "paperh" => s.pgh = v.max(1440),
                        "margl" => s.margins[0] = v,
                        "margr" => s.margins[1] = v,
                        "margt" => s.margins[2] = v,
                        "margb" => s.margins[3] = v,
                        _ => s.gutter = v,
                    }
                }
            }
            "landscape" => {
                self.doc_sect.landscape = true;
                self.sect.landscape = true;
            }
            // ── sections ──
            "sectd" => {
                let hf = std::mem::take(&mut self.sect.hf);
                self.sect = self.doc_sect.clone();
                self.sect.hf = hf;
            }
            "pgwsxn" => self.sect.pgw = n.clamp(1440, 31_680 * 2),
            "pghsxn" => self.sect.pgh = n.clamp(1440, 31_680 * 2),
            "marglsxn" => self.sect.margins[0] = n.clamp(0, 31_680),
            "margrsxn" => self.sect.margins[1] = n.clamp(0, 31_680),
            "margtsxn" => self.sect.margins[2] = n.clamp(-31_680, 31_680),
            "margbsxn" => self.sect.margins[3] = n.clamp(-31_680, 31_680),
            "guttersxn" => self.sect.gutter = n.clamp(0, 31_680),
            "lndscpsxn" => self.sect.landscape = true,
            "titlepg" => self.sect.titlepg = true,
            "headery" => self.sect.header_y = n.clamp(0, 31_680),
            "footery" => self.sect.footer_y = n.clamp(0, 31_680),
            "sbknone" => self.sect.brk = "continuous",
            "sbkcol" => self.sect.brk = "nextColumn",
            "sbkpage" => self.sect.brk = "nextPage",
            "sbkeven" => self.sect.brk = "evenPage",
            "sbkodd" => self.sect.brk = "oddPage",
            "pgnstarts" => self.sect.pgn_start = Some(n),
            "sect" if body && self.sinks.len() == 1 => {
                let s = self.sect.clone();
                // The next section keeps the settings but not the headers
                // (Word's sections inherit those anyway).
                self.sect.hf.clear();
                self.end_para(false, Some(Box::new(s)));
            }
            // ── character formatting ──
            "plain" => {
                let plain = self.plain.clone();
                self.top().c = plain;
            }
            "b" => self.top().c.bold = on,
            "i" => self.top().c.italic = on,
            "strike" => self.top().c.strike = on,
            "striked" => self.top().c.dstrike = on,
            "caps" => self.top().c.caps = on,
            "scaps" => self.top().c.scaps = on,
            "v" => self.top().c.hidden = on,
            "ul" | "uld" | "uldb" | "uldash" | "uldashd" | "uldashdd" | "ulth" | "ulw"
            | "ulwave" | "ulhwave" | "ululdbwave" | "ulldash" | "ulthd" | "ulthdash"
            | "ulthldash" | "ulthdashd" | "ulthdashdd" => {
                let kind = match w {
                    "ul" => "single",
                    "uld" => "dotted",
                    "uldb" => "double",
                    "uldash" => "dash",
                    "uldashd" => "dotDash",
                    "uldashdd" => "dotDotDash",
                    "ulth" => "thick",
                    "ulw" => "words",
                    "ulwave" => "wave",
                    "ulhwave" => "wavyHeavy",
                    "ululdbwave" => "wavyDouble",
                    "ulldash" => "dashLong",
                    "ulthd" => "dottedHeavy",
                    "ulthdash" => "dashedHeavy",
                    "ulthldash" => "dashLongHeavy",
                    "ulthdashd" => "dashDotHeavy",
                    _ => "dashDotDotHeavy",
                };
                self.top().c.ul = on.then_some(kind);
            }
            "ulnone" => self.top().c.ul = None,
            "fs" => self.top().c.size = if n > 0 { n.min(3276) } else { 24 },
            "f" => self.top().c.font = Some(n),
            "cf" => self.top().c.color = n.max(0),
            "highlight" => self.top().c.highlight = n.max(0),
            "cb" | "chcbpat" => self.top().c.bg = n.max(0),
            "super" => self.top().c.valign = 1,
            "sub" => self.top().c.valign = -1,
            "nosupersub" => self.top().c.valign = 0,
            "cs" => self.top().c.style = Some(n),
            "uc" => self.top().uc = n.clamp(0, 10) as u32,
            "u" => {
                let code = (if n < 0 { n + 65536 } else { n }) as u32 & 0xFFFF;
                let ch = match code {
                    0xD800..=0xDBFF => {
                        self.high = Some(code as u16);
                        None
                    }
                    0xDC00..=0xDFFF => self.high.take().and_then(|h| {
                        char::from_u32(0x10000 + ((u32::from(h) - 0xD800) << 10) + (code - 0xDC00))
                    }),
                    _ => {
                        self.high = None;
                        // Symbol-font private-use characters are bytes.
                        char::from_u32(code)
                    }
                };
                if let Some(ch) = ch {
                    self.text(ch.encode_utf8(&mut [0; 4]));
                }
                self.skip = self.stack.last().map_or(1, |s| s.uc);
            }
            // ── special characters ──
            "emdash" => self.text("\u{2014}"),
            "endash" => self.text("\u{2013}"),
            "lquote" => self.text("\u{2018}"),
            "rquote" => self.text("\u{2019}"),
            "ldblquote" => self.text("\u{201C}"),
            "rdblquote" => self.text("\u{201D}"),
            "bullet" => self.text("\u{2022}"),
            "emspace" => self.text("\u{2003}"),
            "enspace" => self.text("\u{2002}"),
            "qmspace" => self.text("\u{2005}"),
            "zwj" => self.text("\u{200D}"),
            "zwnj" => self.text("\u{200C}"),
            "tab" => self.text("\t"),
            "line" | "lbr" if body => {
                let c = self.stack.last().map(|s| s.c.clone()).unwrap_or_default();
                self.sink().inl.push(Inline::Break(c));
            }
            "page" if body => self.sink().inl.push(Inline::PageBreak),
            "chpgn" if body => {
                let c = self.stack.last().map(|s| s.c.clone()).unwrap_or_default();
                self.sink().inl.push(Inline::Field {
                    instr: "PAGE".into(),
                    shown: "1".into(),
                    c,
                });
            }
            "par" if body => self.end_para(false, None),
            "cell" if body => self.end_cell(),
            // Nested tables are flattened: their cells become paragraphs.
            "nestcell" if body => {
                self.notes
                    .insert("tables inside tables were flattened into paragraphs");
                self.end_para(true, None);
            }
            "row" if body => self.end_row(),
            // ── paragraph formatting ──
            "pard" => {
                let pard = self.pard.clone();
                self.top().p = pard;
            }
            "s" => self.top().p.style = n,
            "ql" => self.top().p.align = None,
            "qc" => self.top().p.align = Some("center"),
            "qr" => self.top().p.align = Some("right"),
            "qj" => self.top().p.align = Some("both"),
            "qd" => self.top().p.align = Some("distribute"),
            "li" | "lin" => self.top().p.li = n.clamp(-31_680, 31_680),
            "ri" | "rin" => self.top().p.ri = n.clamp(-31_680, 31_680),
            "fi" => self.top().p.fi = n.clamp(-31_680, 31_680),
            "sb" => self.top().p.sb = n.clamp(0, 31_680),
            "sa" => self.top().p.sa = n.clamp(0, 31_680),
            "sl" => self.top().p.sl = n.clamp(-31_680, 31_680),
            "slmult" => self.top().p.slmult = on,
            "keepn" => self.top().p.keepn = on,
            "keep" => self.top().p.keep = on,
            "pagebb" => self.top().p.pagebb = on,
            "contextualspace" => self.top().p.contextual = on,
            "outlinelevel" => self.top().p.outline = (0..9).contains(&n).then_some(n as u8),
            "ls" => self.top().p.ls = Some(n),
            "ilvl" => self.top().p.ilvl = n.clamp(0, 8),
            "intbl" => self.top().p.intbl = true,
            "itap" => self.top().p.intbl = n > 0,
            "tqr" => self.tab_kind = "right",
            "tqc" => self.tab_kind = "center",
            "tqdec" => self.tab_kind = "decimal",
            "tldot" => self.tab_leader = Some("dot"),
            "tlhyph" => self.tab_leader = Some("hyphen"),
            "tlul" | "tlth" => self.tab_leader = Some("underscore"),
            "tlmdot" => self.tab_leader = Some("middleDot"),
            "tx" | "tb" => {
                let kind = if w == "tb" { "bar" } else { self.tab_kind };
                let tab = TabStop {
                    pos: n.clamp(0, 31_680),
                    kind,
                    leader: self.tab_leader,
                };
                self.tab_kind = "left";
                self.tab_leader = None;
                let tabs = &mut self.top().p.tabs;
                if tabs.len() < 64 {
                    tabs.push(tab);
                }
            }
            // ── tables ──
            "trowd" => {
                self.row = RowDef::default();
                self.cell = CellDef::default();
                self.border = BorderTarget::None;
            }
            "trgaph" => self.row.gaph = n.clamp(0, 31_680),
            "trleft" => self.row.left = n.clamp(-31_680, 31_680),
            "trrh" => self.row.height = n.clamp(-31_680, 31_680),
            "trhdr" => self.row.header = true,
            "trqc" => self.row.jc = Some("center"),
            "trqr" => self.row.jc = Some("right"),
            "trql" => self.row.jc = None,
            "trpaddl" => self.row.pad[0] = Some(n.clamp(0, 31_680)),
            "trpaddr" => self.row.pad[1] = Some(n.clamp(0, 31_680)),
            "clmgf" => self.cell.hmerge = 1,
            "clmrg" => self.cell.hmerge = 2,
            "clvmgf" => self.cell.vmerge = 1,
            "clvmrg" => self.cell.vmerge = 2,
            "clvertalt" => self.cell.valign = None,
            "clvertalc" => self.cell.valign = Some("center"),
            "clvertalb" => self.cell.valign = Some("bottom"),
            "clcbpat" => self.cell.shade = n.max(0),
            "cellx" => {
                let mut cell = std::mem::take(&mut self.cell);
                cell.right = n.clamp(-31_680, 31_680 * 2);
                if self.row.cells.len() < 63 {
                    self.row.cells.push(cell);
                }
                self.border = BorderTarget::None;
            }
            "clbrdrt" => self.border = BorderTarget::Cell(0),
            "clbrdrl" => self.border = BorderTarget::Cell(1),
            "clbrdrb" => self.border = BorderTarget::Cell(2),
            "clbrdrr" => self.border = BorderTarget::Cell(3),
            "trbrdrt" => self.border = BorderTarget::Row(0),
            "trbrdrl" => self.border = BorderTarget::Row(1),
            "trbrdrb" => self.border = BorderTarget::Row(2),
            "trbrdrr" => self.border = BorderTarget::Row(3),
            "trbrdrh" => self.border = BorderTarget::Row(4),
            "trbrdrv" => self.border = BorderTarget::Row(5),
            "brdrt" | "brdrb" | "brdrl" | "brdrr" | "brdrbtw" | "brdrbar" | "box" => {
                self.border = BorderTarget::None;
            }
            "brdrs" | "brdrth" | "brdrsh" | "brdrhair" => self.set_border_kind("single"),
            "brdrdb" => self.set_border_kind("double"),
            "brdrdot" => self.set_border_kind("dotted"),
            "brdrdash" | "brdrdashsm" => self.set_border_kind("dashed"),
            "brdrdashd" => self.set_border_kind("dotDash"),
            "brdrdashdd" => self.set_border_kind("dotDotDash"),
            "brdrtriple" => self.set_border_kind("triple"),
            "brdrwavy" => self.set_border_kind("wave"),
            "brdrnone" | "brdrnil" | "brdrtbl" => {
                let target = self.border;
                match target {
                    BorderTarget::Cell(i) => self.cell.borders[i] = None,
                    BorderTarget::Row(i) => self.row.borders[i] = None,
                    BorderTarget::None => {}
                }
                // Further words for this border are ignored.
                self.border = BorderTarget::None;
            }
            "brdrw" => {
                if let Some(b) = self.border() {
                    b.width = n.clamp(1, 255);
                }
            }
            "brdrcf" => {
                if let Some(b) = self.border() {
                    b.color = n.max(0);
                }
            }
            _ => {
                let _ = param;
            }
        }
    }

    fn into_parsed(mut self) -> Parsed {
        // Text after the last `\par` is a paragraph too.
        while self.sinks.len() > 1 {
            self.sinks.pop();
        }
        let p = self
            .end_p
            .take()
            .or_else(|| self.stack.last().map(|s| s.p.clone()))
            .unwrap_or_default();
        let mut blocks = Self::close_sink(&mut self.sinks[0], p, &self.row);
        if blocks.is_empty() {
            blocks.push(Block::Para(Para::default()));
        }
        Parsed {
            blocks,
            sect: self.sect,
            fonts: self.fonts,
            colors: self.colors,
            styles: self.styles,
            lists: self.lists,
            overrides: self.overrides,
            plain: self.plain,
            pard: self.pard,
            deftab: self.deftab,
            facing: self.facing,
            notes: self.notes,
        }
    }
}

/// A field instruction split into words (quotes grouped, `\x` switches kept).
fn field_words(instr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut had_quote = false;
    for ch in instr.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                had_quote = true;
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() || had_quote {
                    out.push(std::mem::take(&mut cur));
                }
                had_quote = false;
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || had_quote {
        out.push(cur);
    }
    out
}

/// Parse an RTF file. Never fails: what isn't understood is skipped.
pub fn parse(bytes: &[u8]) -> Parsed {
    let mut p = Parser::new();
    p.run(bytes);
    p.into_parsed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn field_words_group_quotes() {
        assert_eq!(
            field_words(r#"HYPERLINK "http://a b/" \l "x""#),
            vec!["HYPERLINK", "http://a b/", "\\l", "x"]
        );
        assert_eq!(field_words("  PAGE  "), vec!["PAGE"]);
    }
}
