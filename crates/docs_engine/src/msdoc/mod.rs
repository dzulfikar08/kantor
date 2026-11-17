//! Importing legacy Word 97-2003 documents (`.doc`, [MS-DOC]).
//!
//! The binary file is read into a small intermediate form (stories of
//! paragraphs and tables with decoded properties) and written out as a
//! WordprocessingML package in memory, which the regular `.docx` reader
//! then turns into a [`crate::Document`]. That keeps one model and one set
//! of rules for styles, lists, tables, sections and pictures, and Save
//! writes an ordinary `.docx`. Kantor never writes `.doc`.
//!
//! What's read:
//!
//! - the compound file ([`cfb`]), the FIB, the piece table (8-bit cp1252
//!   and UTF-16 pieces);
//! - main text, headers and footers, footnotes and endnotes; field
//!   instructions are skipped and their results kept (HYPERLINK becomes a
//!   link, PAGE / NUMPAGES / DATE stay fields);
//! - character formatting (CHPX FKPs), paragraph formatting (PAPX FKPs,
//!   including huge PAPXs in the Data stream), the stylesheet (STSH), fonts
//!   (SttbfFfn), lists (PlfLst / PlfLfo);
//! - tables (row ends, cell boundaries, horizontal and vertical merges,
//!   borders, shading, row heights), tables nested in cells;
//! - sections: page size, orientation, margins, header distances, title
//!   page, page number restarts;
//! - inline PNG and JPEG pictures (PICF + OfficeArt blips in the Data
//!   stream);
//! - floating pictures, shapes and text boxes of the main text ([`art`]:
//!   SPA anchors, the OfficeArt drawing, the text box story), written as
//!   `wp:anchor` objects.
//!
//! - comments (PlcfandRef / PlcfandTxt, authors from GrpXstAtnOwners) with
//!   their ranges (the annotation bookmarks PlcfAtnBkf / PlcfAtnBkl, tagged
//!   in SttbfAtnBkmk) and dates (AtrdExtra, Word 2002 and later).
//!
//! Not imported (listed in [`Imported::notes`]): floating objects in
//! text boxes and notes, grouped shapes, WMF/EMF pictures and embedded objects, hidden text.
//!
//! Tracked insertions and deletions (`sprmCFRMarkIns` / `sprmCFRMarkDel`
//! with their authors from SttbfRMark and dates) come in as revisions
//! (`w:ins` / `w:del`), deleted paragraph marks included.
//!
//! Encrypted files and files from Word 6 / Word 95 are refused with a
//! message. A damaged file is an error, never a panic: every offset is
//! checked.

mod art;
mod cfb;
mod emit;
mod sprm;

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context as _, Result, anyhow, bail};

pub use cfb::is_compound;
use sprm::{Chp, Pap, Sep, Tap};

/// Checked little-endian reads: `None` past the end.
pub(crate) mod rd {
    pub fn u16_at(b: &[u8], off: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            b.get(off..off.checked_add(2)?)?.try_into().ok()?,
        ))
    }
    pub fn i16_at(b: &[u8], off: usize) -> Option<i16> {
        u16_at(b, off).map(|v| v as i16)
    }
    pub fn u32_at(b: &[u8], off: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            b.get(off..off.checked_add(4)?)?.try_into().ok()?,
        ))
    }
    pub fn i32_at(b: &[u8], off: usize) -> Option<i32> {
        u32_at(b, off).map(|v| v as i32)
    }
}
use rd::{i32_at, u16_at, u32_at};

/// A `.doc` read as a `.docx` package.
pub struct Imported {
    /// The WordprocessingML package (a zip), ready for [`crate::docx::read`].
    pub docx: Vec<u8>,
    /// What couldn't be brought over, as short sentences.
    pub notes: Vec<String>,
}

fn damaged() -> anyhow::Error {
    anyhow!("the file is damaged")
}

/// Read a Word 97-2003 document.
pub fn import(bytes: &[u8]) -> Result<Imported> {
    let cf = cfb::Compound::open(bytes)?;
    let word = cf.stream("WordDocument")?.ok_or_else(|| {
        if cf.has_stream("EncryptedPackage") {
            anyhow!("it is password-protected; Kantor can't open encrypted documents")
        } else if cf.has_stream("Workbook") || cf.has_stream("Book") {
            anyhow!("it is an Excel workbook, not a Word document; open it in Sheets")
        } else {
            anyhow!("it is not a Word document")
        }
    })?;
    let fib = Fib::read(&word)?;
    let table = cf
        .stream(if fib.which_table { "1Table" } else { "0Table" })?
        .context("the document's table stream is missing")?;
    let data = cf.stream("Data")?.unwrap_or_default();
    let mut r = Reader::new(fib, word, table, data)?;
    let doc = r.read()?;
    let docx = emit::package(&doc)?;
    Ok(Imported {
        docx,
        notes: r.notes.into_iter().collect(),
    })
}

// ── FIB ───────────────────────────────────────────────────────────────────

/// The parts of the File Information Block the importer uses.
struct Fib {
    which_table: bool,
    /// Character counts of the stories: main text, footnotes, headers,
    /// comments, endnotes, text boxes, header text boxes.
    ccp: [u32; 7],
    /// `(fc, lcb)` pairs of FibRgFcLcb.
    fclcb: Vec<(u32, u32)>,
}

// Indexes into FibRgFcLcb.
const STSHF: usize = 1;
const PLCFFND_REF: usize = 2;
const PLCFFND_TXT: usize = 3;
const PLCFAND_REF: usize = 4;
const PLCFAND_TXT: usize = 5;
const PLCFSED: usize = 6;
const PLCFHDD: usize = 11;
const PLCFBTE_CHPX: usize = 12;
const PLCFBTE_PAPX: usize = 13;
const STTBF_FFN: usize = 15;
const DOP: usize = 31;
const CLX: usize = 33;
const GRP_XST_ATN_OWNERS: usize = 36;
const STTBF_ATN_BKMK: usize = 37;
const PLC_SPA_MOM: usize = 40;
const PLC_SPA_HDR: usize = 41;
const PLCF_ATN_BKF: usize = 42;
const PLCF_ATN_BKL: usize = 43;
const DGG_INFO: usize = 50;
const STTBF_RMARK: usize = 51;
const PLCFTXBX_TXT: usize = 56;
const PLCF_HDRTXBX_TXT: usize = 58;
const PLCFEND_REF: usize = 46;
const PLCFEND_TXT: usize = 47;
const PLF_LST: usize = 73;
const PLF_LFO: usize = 74;
/// FibRgFcLcb2002: comments' dates (ATRDPost10, 18 bytes each).
const ATRD_EXTRA: usize = 112;

impl Fib {
    fn read(w: &[u8]) -> Result<Self> {
        match u16_at(w, 0) {
            Some(0xA5EC) => {}
            // Word 6 and Word 95 files have their own wIdent (and nFib
            // 101-105); they were refused as "not a Word document".
            Some(0xA5DC) => bail!(
                "it was saved by Word 6 or Word 95, which Kantor can't read; save it as .docx in Word or LibreOffice first"
            ),
            _ => bail!("it is not a Word document"),
        }
        let nfib = u16_at(w, 2).ok_or_else(damaged)?;
        let flags = u16_at(w, 0x0A).ok_or_else(damaged)?;
        if nfib < 0x00C0 {
            bail!(
                "it was saved by Word 6 or Word 95, which Kantor can't read; save it as .docx in Word or LibreOffice first"
            );
        }
        if flags & 0x0100 != 0 || flags & 0x8000 != 0 {
            bail!("it is password-protected; Kantor can't open encrypted documents");
        }
        let csw = u16_at(w, 32).ok_or_else(damaged)? as usize;
        let lw_at = 34 + csw * 2;
        let cslw = u16_at(w, lw_at).ok_or_else(damaged)? as usize;
        let lw = |i: usize| -> u32 {
            if i < cslw {
                u32_at(w, lw_at + 2 + i * 4).unwrap_or(0)
            } else {
                0
            }
        };
        // FibRgLw97: ccpText is the 4th field, then Ftn, Hdd, (reserved),
        // Atn, Edn, Txbx, HdrTxbx.
        let ccp = [lw(3), lw(4), lw(5), lw(7), lw(8), lw(9), lw(10)];
        let fc_at = lw_at + 2 + cslw * 4;
        let n = u16_at(w, fc_at).ok_or_else(damaged)? as usize;
        let fclcb = (0..n)
            .map_while(|i| Some((u32_at(w, fc_at + 2 + i * 8)?, u32_at(w, fc_at + 6 + i * 8)?)))
            .collect::<Vec<_>>();
        if fclcb.len() <= CLX {
            return Err(damaged());
        }
        Ok(Self {
            which_table: flags & 0x0200 != 0,
            ccp,
            fclcb,
        })
    }
}

// ── intermediate form ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoteKind {
    Foot,
    End,
}

/// Something in a paragraph.
#[derive(Clone, Debug)]
pub(crate) enum Item {
    /// Text; may hold `\t`, line breaks and page breaks.
    Text(Chp, String),
    /// A picture: media index, size in twips.
    Picture(Chp, usize, u32, u32),
    /// A note reference in the body (note number, 1-based).
    NoteRef(Chp, NoteKind, usize),
    /// The note's own number, at the start of its text.
    NoteMark(Chp, NoteKind),
    LinkStart(String),
    LinkEnd,
    FieldStart(String),
    FieldEnd,
    /// A floating object, at its anchor character.
    Float(Chp, Box<Floating>),
    /// Where comment `n` (0-based) starts and ends, and its reference.
    CommentStart(usize),
    CommentEnd(usize),
    CommentRef(Chp, usize),
}

/// A comment: author, initials, date, text.
pub(crate) struct Comment {
    pub author: String,
    pub initials: String,
    /// When it was written (`2026-11-17T09:30:00Z`), when the file says.
    pub date: Option<String>,
    pub blocks: Vec<Block>,
}

/// A floating picture, shape or text box.
#[derive(Clone, Debug)]
pub(crate) struct Floating {
    pub spa: art::Spa,
    /// Drawing order (higher in front).
    pub z: usize,
    pub name: Option<String>,
    pub descr: Option<String>,
    pub body: FloatBody,
}

#[derive(Clone, Debug)]
pub(crate) enum FloatBody {
    /// Media index and crop (left, top, right, bottom in 1/1000 %).
    Picture {
        media: usize,
        crop: [i64; 4],
    },
    Shape(Box<FloatShape>),
}

/// A shape's outline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FloatLine {
    pub color: u32,
    /// EMU.
    pub width: i64,
    pub dash: bool,
    pub head: Option<&'static str>,
    pub tail: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub(crate) struct FloatShape {
    /// `a:prstGeom` or `a:custGeom`.
    pub geom: String,
    pub fill: Option<u32>,
    pub line: Option<FloatLine>,
    pub flip: (bool, bool),
    /// 60000ths of a degree.
    pub rot: i64,
    pub text_box: bool,
    pub text: Option<Vec<Block>>,
    /// Text insets (EMU): left, top, right, bottom.
    pub insets: [i64; 4],
    pub anchor: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct Para {
    pub pap: Pap,
    /// Formatting of the paragraph mark.
    pub mark: Chp,
    pub items: Vec<Item>,
    /// Ends section `n` (not the last one).
    pub section_end: Option<usize>,
    /// A table nested in a cell, in place of a paragraph.
    pub nested: Option<Vec<Row>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub tap: Tap,
    pub cells: Vec<Vec<Para>>,
}

#[derive(Clone, Debug)]
pub(crate) enum Block {
    Para(Box<Para>),
    Table(Vec<Row>),
}

pub(crate) struct StyleDef {
    pub name: String,
    pub sti: u16,
    /// 1 paragraph, 2 character, 3 table, 4 numbering.
    pub kind: u8,
    pub base: Option<u16>,
    pub next: Option<u16>,
    pub pap: Vec<u8>,
    pub chp: Vec<u8>,
}

pub(crate) struct Level {
    pub start: i32,
    pub nfc: u8,
    pub jc: u8,
    pub legal: bool,
    pub follow: u8,
    /// Number text with level placeholders as `%1`…
    pub text: String,
    pub pap: Vec<u8>,
    pub chp: Vec<u8>,
}

pub(crate) struct List {
    pub lsid: i32,
    pub levels: Vec<Level>,
    pub hybrid: bool,
}

pub(crate) struct Lfo {
    pub lsid: i32,
    /// Start-at overrides: level, value.
    pub starts: Vec<(u8, i32)>,
}

pub(crate) struct Section {
    pub sep: Sep,
    /// Header / footer stories: even header, odd header, even footer, odd
    /// footer, first header, first footer (`None`: same as the previous
    /// section).
    pub hf: [Option<Vec<Block>>; 6],
}

pub(crate) struct Doc {
    pub fonts: Vec<String>,
    pub styles: Vec<Option<StyleDef>>,
    pub default_fonts: [Option<u16>; 3],
    pub lists: Vec<List>,
    pub lfos: Vec<Lfo>,
    pub body: Vec<Block>,
    pub sections: Vec<Section>,
    pub footnotes: Vec<Vec<Block>>,
    pub endnotes: Vec<Vec<Block>>,
    pub comments: Vec<Comment>,
    /// Pictures: extension (`png` / `jpeg`) and bytes.
    pub media: Vec<(&'static str, Vec<u8>)>,
    pub facing_pages: bool,
    pub default_tab: u16,
    /// Who made tracked changes (SttbfRMark).
    pub revision_authors: Vec<String>,
}

// ── reading ───────────────────────────────────────────────────────────────

/// FKP property runs over file offsets.
struct Runs<T> {
    runs: Vec<(u32, u32, T)>,
}

impl<T> Runs<T> {
    fn find(&self, fc: u32) -> Option<&T> {
        let i = self.runs.partition_point(|r| r.0 <= fc);
        let r = self.runs.get(i.checked_sub(1)?)?;
        (fc < r.1).then_some(&r.2)
    }
}

/// Byte → Unicode for 8-bit pieces (Windows-1252).
fn cp1252(b: u8) -> u16 {
    const HIGH: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013,
        0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as u16,
    }
}

/// Upper bound on characters, against damaged piece tables.
const MAX_CHARS: u32 = 64 << 20;

struct Reader {
    fib: Fib,
    word: Vec<u8>,
    table: Vec<u8>,
    data: Vec<u8>,
    /// Text by CP and the file offset of each character.
    units: Vec<u16>,
    fcs: Vec<u32>,
    chpx: Runs<Vec<u8>>,
    papx: Runs<Vec<u8>>,
    /// Section boundaries (CP after each section's last character).
    sed_cps: Vec<u32>,
    foot_refs: HashMap<u32, usize>,
    end_refs: HashMap<u32, usize>,
    media: Vec<(&'static str, Vec<u8>)>,
    notes: BTreeSet<String>,
    /// Floating objects of the main text by anchor CP, their shapes and
    /// the text box ranges.
    spas: HashMap<u32, art::Spa>,
    hdr_spas: HashMap<u32, art::Spa>,
    drawing: art::Drawing,
    text_boxes: Vec<(u32, u32, u32)>,
    hdr_text_boxes: Vec<(u32, u32, u32)>,
    /// Reading a header's or footer's floating object.
    in_header: bool,
    /// Media index of each BLIP store picture already added.
    blip_media: HashMap<usize, usize>,
    /// Text box stories being read (no floating objects in them).
    in_text_box: usize,
    /// Comments by reference CP, and their ranges' starts and ends by CP.
    comment_refs: HashMap<u32, usize>,
    comment_marks: HashMap<u32, Vec<Item>>,
}

impl Reader {
    fn new(fib: Fib, word: Vec<u8>, table: Vec<u8>, data: Vec<u8>) -> Result<Self> {
        let mut r = Self {
            fib,
            word,
            table,
            data,
            units: Vec::new(),
            fcs: Vec::new(),
            chpx: Runs { runs: Vec::new() },
            papx: Runs { runs: Vec::new() },
            sed_cps: Vec::new(),
            foot_refs: HashMap::new(),
            end_refs: HashMap::new(),
            media: Vec::new(),
            notes: BTreeSet::new(),
            spas: HashMap::new(),
            hdr_spas: HashMap::new(),
            drawing: art::Drawing::default(),
            text_boxes: Vec::new(),
            hdr_text_boxes: Vec::new(),
            in_header: false,
            blip_media: HashMap::new(),
            in_text_box: 0,
            comment_refs: HashMap::new(),
            comment_marks: HashMap::new(),
        };
        r.read_text()?;
        r.read_fkps()?;
        Ok(r)
    }

    /// A structure in the table stream, `None` when absent.
    fn tbl(&self, i: usize) -> Option<&[u8]> {
        let &(fc, lcb) = self.fib.fclcb.get(i)?;
        if lcb == 0 {
            return None;
        }
        self.table
            .get(fc as usize..(fc as usize).checked_add(lcb as usize)?)
    }

    fn read_text(&mut self) -> Result<()> {
        let clx = self.tbl(CLX).context("the document has no text")?;
        // Skip Prcs (0x01 records), find the Pcdt (0x02).
        let mut i = 0;
        let plc = loop {
            match clx.get(i) {
                Some(1) => {
                    let cb = u16_at(clx, i + 1).ok_or_else(damaged)? as usize;
                    i += 3 + cb;
                }
                Some(2) => {
                    let lcb = u32_at(clx, i + 1).ok_or_else(damaged)? as usize;
                    break clx.get(i + 5..i + 5 + lcb).ok_or_else(damaged)?;
                }
                _ => return Err(damaged()),
            }
        };
        if plc.len() < 4 || (plc.len() - 4) % 12 != 0 {
            return Err(damaged());
        }
        let n = (plc.len() - 4) / 12;
        let cp = |k: usize| u32_at(plc, k * 4).unwrap_or(0);
        let mut units = Vec::new();
        let mut fcs = Vec::new();
        for k in 0..n {
            let (start, end) = (cp(k), cp(k + 1));
            if end < start || end > MAX_CHARS || start as usize != units.len() {
                return Err(damaged());
            }
            let pcd = (n + 1) * 4 + k * 8;
            let fc_raw = u32_at(plc, pcd + 2).ok_or_else(damaged)?;
            let len = (end - start) as usize;
            if fc_raw & 0x4000_0000 != 0 {
                let fc = ((fc_raw & 0x3FFF_FFFF) / 2) as usize;
                let bytes = self.word.get(fc..fc + len).ok_or_else(damaged)?;
                units.extend(bytes.iter().map(|&b| cp1252(b)));
                fcs.extend((0..len).map(|j| (fc + j) as u32));
            } else {
                let fc = fc_raw as usize;
                let bytes = self.word.get(fc..fc + len * 2).ok_or_else(damaged)?;
                units.extend(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c)),
                );
                fcs.extend((0..len).map(|j| (fc + j * 2) as u32));
            }
        }
        self.units = units;
        self.fcs = fcs;
        Ok(())
    }

    fn read_fkps(&mut self) -> Result<()> {
        let pns = |plc: Option<&[u8]>| -> Vec<u32> {
            let Some(plc) = plc else { return Vec::new() };
            if plc.len() < 4 {
                return Vec::new();
            }
            let n = (plc.len() - 4) / 8;
            (0..n)
                .filter_map(|k| u32_at(plc, (n + 1) * 4 + k * 4).map(|p| p & 0x3F_FFFF))
                .collect()
        };
        let mut chpx = Vec::new();
        for pn in pns(self.tbl(PLCFBTE_CHPX)) {
            let Some(fkp) = self.word.get(pn as usize * 512..pn as usize * 512 + 512) else {
                continue;
            };
            let crun = fkp[511] as usize;
            if 4 * (crun + 1) + crun > 511 {
                continue;
            }
            for k in 0..crun {
                let (Some(a), Some(b)) = (u32_at(fkp, k * 4), u32_at(fkp, k * 4 + 4)) else {
                    break;
                };
                let off = fkp[4 * (crun + 1) + k] as usize * 2;
                let g = if off == 0 {
                    Vec::new()
                } else {
                    let cb = *fkp.get(off).unwrap_or(&0) as usize;
                    fkp.get(off + 1..off + 1 + cb).unwrap_or_default().to_vec()
                };
                chpx.push((a, b, g));
            }
        }
        let mut papx = Vec::new();
        for pn in pns(self.tbl(PLCFBTE_PAPX)) {
            let Some(fkp) = self.word.get(pn as usize * 512..pn as usize * 512 + 512) else {
                continue;
            };
            let cpara = fkp[511] as usize;
            if 4 * (cpara + 1) + 13 * cpara > 511 {
                continue;
            }
            for k in 0..cpara {
                let (Some(a), Some(b)) = (u32_at(fkp, k * 4), u32_at(fkp, k * 4 + 4)) else {
                    break;
                };
                let off = fkp[4 * (cpara + 1) + k * 13] as usize * 2;
                let g = if off == 0 {
                    Vec::new()
                } else {
                    let cb = *fkp.get(off).unwrap_or(&0) as usize;
                    let (at, len) = if cb != 0 {
                        (off + 1, cb * 2 - 1)
                    } else {
                        (off + 2, *fkp.get(off + 1).unwrap_or(&0) as usize * 2)
                    };
                    fkp.get(at..at + len).unwrap_or_default().to_vec()
                };
                papx.push((a, b, g));
            }
        }
        chpx.sort_by_key(|r| r.0);
        papx.sort_by_key(|r| r.0);
        self.chpx = Runs { runs: chpx };
        self.papx = Runs { runs: papx };
        Ok(())
    }

    fn chp_at(&self, cp: usize) -> Chp {
        let mut c = Chp::default();
        if let Some(g) = self.fcs.get(cp).and_then(|&fc| self.chpx.find(fc)) {
            c.apply(g);
        }
        c
    }

    fn pap_at(&self, cp: usize) -> Pap {
        let mut p = Pap::default();
        let Some(g) = self.fcs.get(cp).and_then(|&fc| self.papx.find(fc)) else {
            return p;
        };
        if g.len() >= 2 {
            p.istd = u16_at(g, 0).unwrap_or(0);
            p.apply(&g[2..]);
        }
        if let Some(at) = p.huge.take() {
            let at = at as usize;
            if let Some(cb) = u16_at(&self.data, at)
                && let Some(g) = self.data.get(at + 2..at + 2 + cb as usize)
            {
                p.apply(g);
            }
        }
        p
    }

    fn note(&mut self, s: &str) {
        self.notes.insert(s.to_owned());
    }

    fn read(&mut self) -> Result<Doc> {
        let ccp = self.fib.ccp;
        let main_end = (ccp[0] as usize).min(self.units.len());
        self.spas = self.tbl(PLC_SPA_MOM).map(art::spas).unwrap_or_default();
        // Headers' and footers' objects: their CPs count from the start of
        // the header text.
        self.hdr_spas = self.tbl(PLC_SPA_HDR).map(art::spas).unwrap_or_default();
        if !self.spas.is_empty() || !self.hdr_spas.is_empty() {
            self.drawing = self
                .tbl(DGG_INFO)
                .map(|d| art::drawing(d, &self.word))
                .unwrap_or_default();
            self.text_boxes = self
                .tbl(PLCFTXBX_TXT)
                .map(art::text_boxes)
                .unwrap_or_default();
            self.hdr_text_boxes = self
                .tbl(PLCF_HDRTXBX_TXT)
                .map(art::text_boxes)
                .unwrap_or_default();
        }
        let fonts = self.fonts();
        let (styles, default_fonts) = self.styles();
        let (lists, lfos) = self.lists();
        // Sections.
        let mut seps = Vec::new();
        if let Some(plc) = self.tbl(PLCFSED).map(<[u8]>::to_vec)
            && plc.len() >= 4
        {
            let n = (plc.len() - 4) / 16;
            for k in 0..n {
                let cp = u32_at(&plc, (k + 1) * 4).unwrap_or(0);
                let sed = (n + 1) * 4 + k * 12;
                let fc = i32_at(&plc, sed + 2).unwrap_or(-1);
                let mut sep = Sep::default();
                if fc >= 0
                    && let Some(cb) = u16_at(&self.word, fc as usize)
                    && let Some(g) = self
                        .word
                        .get(fc as usize + 2..fc as usize + 2 + cb as usize)
                {
                    sep.apply(g);
                }
                self.sed_cps.push(cp);
                seps.push(sep);
            }
        }
        if seps.is_empty() {
            seps.push(Sep::default());
            self.sed_cps.push(main_end as u32);
        }
        // Notes.
        let foot_txt = self.note_texts(PLCFFND_REF, PLCFFND_TXT, ccp[0], NoteKind::Foot);
        let end_base = ccp[0] + ccp[1] + ccp[2] + ccp[3];
        let end_txt = self.note_texts(PLCFEND_REF, PLCFEND_TXT, end_base, NoteKind::End);

        let comment_texts = if ccp[3] > 0 {
            self.comment_ranges(ccp[0] + ccp[1] + ccp[2])
        } else {
            Vec::new()
        };
        let body = self.story(0, main_end, true);
        let mut comments = Vec::new();
        let dates: Vec<Option<String>> = self.tbl(ATRD_EXTRA).map_or_else(Vec::new, |x| {
            x.as_chunks::<18>()
                .0
                .iter()
                .map(|c| dttm(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                .collect()
        });
        for (k, (author, initials, a, b)) in comment_texts.into_iter().enumerate() {
            let mut blocks = self.story(a, b, false);
            if blocks.len() > 1
                && matches!(blocks.last(), Some(Block::Para(p)) if p.items.is_empty())
            {
                blocks.pop();
            }
            comments.push(Comment {
                author,
                initials,
                date: dates.get(k).cloned().flatten(),
                blocks,
            });
        }
        let mut footnotes = Vec::new();
        for (a, b) in foot_txt {
            footnotes.push(self.story(a, b, false));
        }
        let mut endnotes = Vec::new();
        for (a, b) in end_txt {
            endnotes.push(self.story(a, b, false));
        }
        // Headers and footers.
        let hdd: Vec<u32> = self
            .tbl(PLCFHDD)
            .map(|p| (0..p.len() / 4).filter_map(|k| u32_at(p, k * 4)).collect())
            .unwrap_or_default();
        let hdr_base = (ccp[0] + ccp[1]) as usize;
        let mut sections = Vec::new();
        for (s, sep) in seps.into_iter().enumerate() {
            let mut hf: [Option<Vec<Block>>; 6] = Default::default();
            for (t, slot) in hf.iter_mut().enumerate() {
                let i = 6 + s * 6 + t;
                if let (Some(&a), Some(&b)) = (hdd.get(i), hdd.get(i + 1))
                    && b > a
                {
                    let (a, b) = (hdr_base + a as usize, hdr_base + b as usize);
                    if b <= self.units.len() {
                        let mut blocks = self.story(a, b, false);
                        // Each story ends with a guard paragraph mark.
                        if blocks.len() > 1
                            && matches!(blocks.last(), Some(Block::Para(p)) if p.items.is_empty())
                        {
                            blocks.pop();
                        }
                        *slot = Some(blocks);
                    }
                }
            }
            sections.push(Section { sep, hf });
        }
        let dop = self.tbl(DOP).unwrap_or_default();
        let facing_pages = dop.first().is_some_and(|b| b & 1 != 0);
        let default_tab = u16_at(dop, 10)
            .filter(|t| (36..=14400).contains(t))
            .unwrap_or(720);
        Ok(Doc {
            fonts,
            styles,
            default_fonts,
            lists,
            lfos,
            body,
            sections,
            footnotes,
            endnotes,
            comments,
            media: std::mem::take(&mut self.media),
            facing_pages,
            default_tab,
            revision_authors: self.tbl(STTBF_RMARK).map(sttb_strings).unwrap_or_default(),
        })
    }

    /// Note text ranges (CPs in the whole text) and the body references.
    fn note_texts(
        &mut self,
        refs: usize,
        txt: usize,
        base: u32,
        kind: NoteKind,
    ) -> Vec<(usize, usize)> {
        let (Some(r), Some(t)) = (self.tbl(refs), self.tbl(txt)) else {
            return Vec::new();
        };
        if r.len() < 4 {
            return Vec::new();
        }
        let n = (r.len() - 4) / 6;
        let ref_cps: Vec<u32> = (0..n).filter_map(|k| u32_at(r, k * 4)).collect();
        let txt_cps: Vec<u32> = (0..t.len() / 4).filter_map(|k| u32_at(t, k * 4)).collect();
        let mut out = Vec::new();
        for (k, cp) in ref_cps.into_iter().enumerate() {
            let (Some(&a), Some(&b)) = (txt_cps.get(k), txt_cps.get(k + 1)) else {
                break;
            };
            let (a, b) = ((base + a) as usize, (base + b) as usize);
            if a > b || b > self.units.len() {
                break;
            }
            match kind {
                NoteKind::Foot => self.foot_refs.insert(cp, out.len() + 1),
                NoteKind::End => self.end_refs.insert(cp, out.len() + 1),
            };
            out.push((a, b));
        }
        out
    }

    /// The comments: author, initials and text range (CPs in the whole
    /// text); fills in their references and ranges in the main text.
    fn comment_ranges(&mut self, base: u32) -> Vec<(String, String, usize, usize)> {
        let (Some(r), Some(t)) = (
            self.tbl(PLCFAND_REF).map(<[u8]>::to_vec),
            self.tbl(PLCFAND_TXT),
        ) else {
            self.note("comments not imported");
            return Vec::new();
        };
        let r = &r[..];
        if r.len() < 4 {
            return Vec::new();
        }
        let n = (r.len() - 4) / 34;
        let txt: Vec<u32> = (0..t.len() / 4).filter_map(|k| u32_at(t, k * 4)).collect();
        // Authors: Xst strings one after another.
        let mut owners = Vec::new();
        if let Some(o) = self.tbl(GRP_XST_ATN_OWNERS) {
            let mut at = 0;
            while let Some(cch) = u16_at(o, at) {
                let units: Vec<u16> = (0..cch as usize)
                    .map_while(|k| u16_at(o, at + 2 + k * 2))
                    .collect();
                owners.push(String::from_utf16_lossy(&units));
                at += 2 + cch as usize * 2;
            }
        }
        // Ranges: bookmark k (tagged in SttbfAtnBkmk) from Bkf[k] to
        // Bkl[ibkl].
        let mut ranges: HashMap<i32, (u32, u32)> = HashMap::new();
        if let (Some(names), Some(bkf), Some(bkl)) = (
            self.tbl(STTBF_ATN_BKMK),
            self.tbl(PLCF_ATN_BKF),
            self.tbl(PLCF_ATN_BKL),
        ) && u16_at(names, 0) == Some(0xFFFF)
            && bkf.len() >= 4
        {
            let count = u16_at(names, 2).unwrap_or(0) as usize;
            let extra = u16_at(names, 4).unwrap_or(0) as usize;
            let nb = (bkf.len() - 4) / 8;
            let mut at = 6;
            for k in 0..count {
                let Some(cch) = u16_at(names, at) else { break };
                at += 2 + cch as usize * 2;
                let tag = i32_at(names, at + 2);
                at += extra;
                let (Some(tag), true) = (tag, k < nb) else {
                    continue;
                };
                let start = u32_at(bkf, k * 4);
                let ibkl = u16_at(bkf, (nb + 1) * 4 + k * 4).unwrap_or(0) as usize;
                let end = u32_at(bkl, ibkl * 4);
                if let (Some(a), Some(b)) = (start, end)
                    && a <= b
                {
                    ranges.insert(tag, (a, b));
                }
            }
        }
        let mut out = Vec::new();
        for k in 0..n {
            let (Some(cp), Some(&a), Some(&b)) = (u32_at(r, k * 4), txt.get(k), txt.get(k + 1))
            else {
                break;
            };
            let at = (n + 1) * 4 + k * 30;
            let initials: Vec<u16> = (0..u16_at(r, at).unwrap_or(0).min(9) as usize)
                .map_while(|j| u16_at(r, at + 2 + j * 2))
                .collect();
            let ibst = u16_at(r, at + 20).map(|v| v as i16).unwrap_or(-1);
            let tag = i32_at(r, at + 26).unwrap_or(-1);
            let (a, b) = ((base + a) as usize, (base + b) as usize);
            if a > b || b > self.units.len() {
                break;
            }
            let id = out.len();
            self.comment_refs.insert(cp, id);
            let (start, end) = ranges.get(&tag).copied().unwrap_or((cp, cp));
            self.comment_marks
                .entry(start)
                .or_default()
                .push(Item::CommentStart(id));
            self.comment_marks
                .entry(end)
                .or_default()
                .insert(0, Item::CommentEnd(id));
            let author = usize::try_from(ibst)
                .ok()
                .and_then(|i| owners.get(i).cloned())
                .unwrap_or_default();
            out.push((author, String::from_utf16_lossy(&initials), a, b));
        }
        out
    }

    fn fonts(&self) -> Vec<String> {
        let Some(s) = self.tbl(STTBF_FFN) else {
            return Vec::new();
        };
        let n = u16_at(s, 0).unwrap_or(0) as usize;
        let mut out = Vec::new();
        let mut at = 4;
        for _ in 0..n {
            let Some(&cb) = s.get(at) else { break };
            let entry = s.get(at + 1..at + 1 + cb as usize).unwrap_or_default();
            let name: Vec<u16> = (0..)
                .map_while(|k| u16_at(entry, 39 + k * 2).filter(|&c| c != 0))
                .collect();
            out.push(String::from_utf16_lossy(&name));
            at += 1 + cb as usize;
        }
        out
    }

    fn styles(&self) -> (Vec<Option<StyleDef>>, [Option<u16>; 3]) {
        let Some(s) = self.tbl(STSHF) else {
            return (Vec::new(), [None; 3]);
        };
        let cb_stshi = u16_at(s, 0).unwrap_or(0) as usize;
        let cstd = u16_at(s, 2).unwrap_or(0) as usize;
        let cb_base = u16_at(s, 4).unwrap_or(10) as usize;
        let fonts = [u16_at(s, 14), u16_at(s, 16), u16_at(s, 18)];
        let mut out = Vec::with_capacity(cstd);
        let mut at = 2 + cb_stshi;
        for _ in 0..cstd {
            let Some(cb) = u16_at(s, at) else { break };
            at += 2;
            let Some(std) = s.get(at..at + cb as usize) else {
                break;
            };
            at += cb as usize;
            out.push(if cb == 0 {
                None
            } else {
                parse_std(std, cb_base)
            });
        }
        (out, fonts)
    }

    fn lists(&self) -> (Vec<List>, Vec<Lfo>) {
        let mut lists = Vec::new();
        // The LVLs follow the PlfLst, outside its byte count.
        let lst = self
            .tbl(PLF_LST)
            .and_then(|_| self.table.get(self.fib.fclcb[PLF_LST].0 as usize..));
        if let Some(s) = lst {
            let n = u16_at(s, 0).unwrap_or(0) as usize;
            let mut at = 2 + n * 28;
            for k in 0..n {
                let base = 2 + k * 28;
                let (Some(lsid), Some(&flags)) = (i32_at(s, base), s.get(base + 26)) else {
                    break;
                };
                let count = if flags & 1 != 0 { 1 } else { 9 };
                let mut levels = Vec::new();
                for _ in 0..count {
                    let Some((lvl, next)) = parse_lvl(s, at) else {
                        return (lists, Vec::new());
                    };
                    levels.push(lvl);
                    at = next;
                }
                lists.push(List {
                    lsid,
                    levels,
                    hybrid: flags & 0x10 != 0,
                });
            }
        }
        let mut lfos = Vec::new();
        if let Some(s) = self.tbl(PLF_LFO) {
            let n = (u32_at(s, 0).unwrap_or(0) as usize).min(s.len() / 16);
            let mut counts = Vec::new();
            for k in 0..n {
                let base = 4 + k * 16;
                let (Some(lsid), Some(&c)) = (i32_at(s, base), s.get(base + 12)) else {
                    break;
                };
                lfos.push(Lfo {
                    lsid,
                    starts: Vec::new(),
                });
                counts.push(c as usize);
            }
            let mut at = 4 + n * 16;
            'lfo: for (k, &c) in counts.iter().enumerate() {
                at += 4; // cp
                for _ in 0..c {
                    let (Some(start), Some(flags)) = (i32_at(s, at), u32_at(s, at + 4)) else {
                        break 'lfo;
                    };
                    at += 8;
                    if flags & 0x10 != 0 {
                        lfos[k].starts.push(((flags & 0xF) as u8, start));
                    }
                    if flags & 0x20 != 0 {
                        match parse_lvl(s, at) {
                            Some((_, next)) => at = next,
                            None => break 'lfo,
                        }
                    }
                }
            }
        }
        (lists, lfos)
    }

    /// Where `cp` is a section's last character, that section's index.
    fn section_end_at(&self, cp: usize) -> Option<usize> {
        self.sed_cps.iter().position(|&s| s as usize == cp + 1)
    }

    /// A picture from the Data stream, as an item.
    fn picture(&mut self, chp: &Chp) -> Option<Item> {
        let at = chp.pic? as usize;
        let d = &self.data;
        let lcb = u32_at(d, at)? as usize;
        let cb_header = u16_at(d, at + 4)? as usize;
        let end = at.checked_add(lcb)?.min(d.len());
        let mm = u16_at(d, at + 6)?;
        let (dxa, dya) = (u16_at(d, at + 28)? as u32, u16_at(d, at + 30)? as u32);
        let (mx, my) = (u16_at(d, at + 32)? as u32, u16_at(d, at + 34)? as u32);
        let mut p = at + cb_header;
        if mm == 0x66 {
            p += 1 + *d.get(p)? as usize;
        }
        // OfficeArt records: the shape container, then blip store entries.
        let mut found = None;
        while p + 8 <= end {
            let typ = u16_at(d, p + 2)?;
            let len = u32_at(d, p + 4)? as usize;
            let body = p + 8;
            let next = body.checked_add(len)?.min(end);
            if typ == 0xF007 {
                // FBSE: 36 bytes, a name, then the blip record.
                let name = *d.get(body + 33)? as usize;
                let blip = body + 36 + name;
                if let Some(len) = u32_at(d, blip + 4) {
                    let bytes = d.get(blip + 8..(blip + 8 + len as usize).min(end))?;
                    found = sniff_image(bytes);
                }
            } else if (0xF018..=0xF117).contains(&typ) {
                found = sniff_image(d.get(body..next)?);
            }
            if found.is_some() || next <= p {
                break;
            }
            p = next;
        }
        let Some((ext, bytes)) = found else {
            self.note("pictures in formats other than PNG and JPEG not imported");
            return None;
        };
        let scale = |v: u32, m: u32| if m == 0 { v } else { v * m / 1000 };
        let (w, h) = (scale(dxa, mx).max(1), scale(dya, my).max(1));
        self.media.push((ext, bytes));
        Some(Item::Picture(chp.visible(), self.media.len() - 1, w, h))
    }

    /// The floating object of `spa`; `None` (with a note) when it can't be
    /// brought in.
    fn floating(&mut self, spa: art::Spa) -> Option<Floating> {
        let Some(shape) = self.drawing.shapes.get(&spa.spid).cloned() else {
            self.note("floating pictures and shapes not imported");
            return None;
        };
        if shape.group() {
            self.note("grouped shapes not imported");
            return None;
        }
        let z = shape.order;
        let name = shape.text(0x380);
        let descr = shape.text(0x381);
        let fixed = |v: u32| (f64::from(v as i32) / 65536.0 * 100_000.0).round() as i64;
        let body = if let Some(pib) = shape.prop(0x104).filter(|_| shape.typ != 202) {
            let i = (pib as usize).checked_sub(1)?;
            match self.drawing.blips.get(i) {
                Some(art::Blip::Image(ext, bytes)) => {
                    let media = match self.blip_media.get(&i) {
                        Some(m) => *m,
                        None => {
                            self.media.push((ext, bytes.clone()));
                            self.blip_media.insert(i, self.media.len() - 1);
                            self.media.len() - 1
                        }
                    };
                    let c = |id: u16| shape.prop(id).map_or(0, fixed);
                    FloatBody::Picture {
                        media,
                        crop: [c(0x102), c(0x100), c(0x103), c(0x101)],
                    }
                }
                _ => {
                    self.note("pictures in formats other than PNG and JPEG not imported");
                    return None;
                }
            }
        } else {
            let geom = match art::preset(shape.typ) {
                Some(p) => format!("<a:prstGeom prst=\"{p}\"><a:avLst/></a:prstGeom>"),
                None => match art::cust_geom(&shape) {
                    Some(g) => g,
                    None if shape.typ == 0 || shape.typ == 4095 => {
                        // A path Kantor can't follow: its box, so the
                        // shape (and its text) still comes in.
                        self.note("some freeform shapes drawn as rectangles");
                        "<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>".to_owned()
                    }
                    None => "<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>".to_owned(),
                },
            };
            let is_line = matches!(shape.typ, 20 | 32);
            let text_box = shape.typ == 202;
            let filled = shape.flag(0x1BF, 4).unwrap_or(!is_line);
            let fill = filled.then(|| shape.prop(0x181).and_then(art::color).unwrap_or(0xFFFFFF));
            let lined = shape.flag(0x1FF, 3).unwrap_or(true);
            let line = lined.then(|| FloatLine {
                color: shape.prop(0x1C0).and_then(art::color).unwrap_or(0),
                width: shape.prop(0x1CB).map_or(9525, i64::from),
                dash: shape.prop(0x1CE).is_some_and(|d| d != 0),
                head: shape.prop(0x1D0).and_then(art::arrowhead),
                tail: shape.prop(0x1D1).and_then(art::arrowhead),
            });
            // The text box story: by shape id, or by the id's index.
            let boxes = if self.in_header {
                &self.hdr_text_boxes
            } else {
                &self.text_boxes
            };
            let range = boxes
                .iter()
                .find(|t| t.2 == spa.spid)
                .or_else(|| {
                    let i = (shape.txid? >> 16) as usize;
                    boxes.get(i.checked_sub(1)?)
                })
                .copied()
                .filter(|_| shape.txid.is_some() || text_box);
            let text = range.map(|(a, b, _)| {
                let c = self.fib.ccp;
                let mut base = (c[0] + c[1] + c[2] + c[3] + c[4]) as usize;
                if self.in_header {
                    base += c[5] as usize;
                }
                self.in_text_box += 1;
                let mut blocks = self.story(base + a as usize, base + b as usize, false);
                self.in_text_box -= 1;
                // An empty last paragraph (LibreOffice writes one).
                if blocks.len() > 1
                    && matches!(blocks.last(), Some(Block::Para(p)) if p.items.is_empty())
                {
                    blocks.pop();
                }
                blocks
            });
            let ins = |id: u16, d: i64| shape.prop(id).map_or(d, |v| i64::from(v as i32));
            FloatBody::Shape(Box::new(FloatShape {
                geom,
                fill,
                line,
                flip: shape.flip(),
                rot: shape.prop(0x04).map_or(0, |v| {
                    (f64::from(v as i32) / 65536.0 * 60_000.0).round() as i64
                }),
                text_box,
                text,
                insets: [
                    ins(0x81, 91440),
                    ins(0x82, 45720),
                    ins(0x83, 91440),
                    ins(0x84, 45720),
                ],
                anchor: match shape.prop(0x87) {
                    Some(1 | 4 | 7) => "ctr",
                    Some(2 | 5 | 8) => "b",
                    _ => "t",
                },
            }))
        };
        Some(Floating {
            spa,
            z,
            name,
            descr,
            body,
        })
    }

    /// The blocks of the text `start..end`.
    fn story(&mut self, start: usize, end: usize, main: bool) -> Vec<Block> {
        let paras = self.paragraphs(start, end, main);
        assemble(paras, &mut self.notes)
    }

    /// Paragraphs of `start..end`, with fields resolved and tables still
    /// flat (their marks in the PAPs).
    fn paragraphs(&mut self, start: usize, end: usize, main: bool) -> Vec<(Para, u16)> {
        struct Field {
            instr: String,
            result: bool,
            /// What the result was wrapped in: 1 link, 2 field.
            wrap: u8,
        }
        let end = end.min(self.units.len());
        let mut out = Vec::new();
        let mut items: Vec<Item> = Vec::new();
        let mut fields: Vec<Field> = Vec::new();
        let mut cp = start;
        let push_text = |items: &mut Vec<Item>, chp: Chp, s: &str| {
            if let Some(Item::Text(c, t)) = items.last_mut()
                && *c == chp
            {
                t.push_str(s);
                return;
            }
            items.push(Item::Text(chp, s.to_owned()));
        };
        while cp < end {
            if main && let Some(marks) = self.comment_marks.remove(&(cp as u32)) {
                items.extend(marks);
            }
            let u = self.units[cp];
            let chp = self.chp_at(cp);
            let hidden_code = fields.iter().any(|f| !f.result);
            match u {
                0x13 => {
                    fields.push(Field {
                        instr: String::new(),
                        result: false,
                        wrap: 0,
                    });
                    cp += 1;
                    continue;
                }
                0x14 => {
                    if let Some(f) = fields.last_mut()
                        && !f.result
                    {
                        f.result = true;
                        let outer_code = fields.iter().rev().skip(1).any(|f| !f.result);
                        if !outer_code {
                            let f = fields.last_mut().expect("just checked");
                            let instr = f.instr.trim().to_owned();
                            let word = instr
                                .split_whitespace()
                                .next()
                                .unwrap_or("")
                                .to_ascii_uppercase();
                            if word == "HYPERLINK"
                                && let Some(url) = hyperlink_url(&instr)
                            {
                                f.wrap = 1;
                                items.push(Item::LinkStart(url));
                            } else if matches!(
                                word.as_str(),
                                "PAGE" | "NUMPAGES" | "SECTIONPAGES" | "DATE" | "TIME"
                            ) {
                                f.wrap = 2;
                                items.push(Item::FieldStart(instr));
                            }
                        }
                    }
                    cp += 1;
                    continue;
                }
                0x15 => {
                    if let Some(f) = fields.pop() {
                        match f.wrap {
                            1 => items.push(Item::LinkEnd),
                            2 => items.push(Item::FieldEnd),
                            _ => {}
                        }
                    }
                    cp += 1;
                    continue;
                }
                _ => {}
            }
            let para_end = matches!(u, 0x0D | 0x07)
                || (u == 0x0C && main && self.section_end_at(cp).is_some());
            if hidden_code && !para_end {
                if let Some(f) = fields.last_mut()
                    && !f.result
                    && f.instr.len() < 4096
                    && let Some(c) = char::from_u32(u as u32)
                {
                    f.instr.push(c);
                }
                cp += 1;
                continue;
            }
            if para_end {
                let pap = self.pap_at(cp);
                let section_end = if main { self.section_end_at(cp) } else { None };
                out.push((
                    Para {
                        pap,
                        mark: chp.visible(),
                        items: std::mem::take(&mut items),
                        section_end: section_end.filter(|&s| s + 1 < self.sed_cps.len()),
                        nested: None,
                    },
                    u,
                ));
                cp += 1;
                continue;
            }
            if chp.vanish == Some(1) {
                self.note("hidden text left out");
                cp += 1;
                continue;
            }
            let vis = chp.visible();
            match u {
                0x09 => push_text(&mut items, vis, "\t"),
                0x0B => push_text(&mut items, vis, &crate::LINE_BREAK.to_string()),
                0x0C => push_text(&mut items, vis, &crate::PAGE_BREAK.to_string()),
                0x0E => push_text(&mut items, vis, &crate::LINE_BREAK.to_string()),
                0x1E => push_text(&mut items, vis, "\u{2011}"),
                0x1F => push_text(&mut items, vis, "\u{AD}"),
                0x01 if chp.special && !chp.data => {
                    if chp.ole2 {
                        self.note("embedded objects not imported");
                    } else if let Some(item) = self.picture(&chp) {
                        items.push(item);
                    }
                }
                0x02 if chp.special => {
                    let r = cp as u32;
                    if let Some(&n) = self.foot_refs.get(&r) {
                        items.push(Item::NoteRef(vis, NoteKind::Foot, n));
                    } else if let Some(&n) = self.end_refs.get(&r) {
                        items.push(Item::NoteRef(vis, NoteKind::End, n));
                    } else if !main {
                        let kind = if (cp as u32) < self.fib.ccp[0] + self.fib.ccp[1] {
                            NoteKind::Foot
                        } else {
                            NoteKind::End
                        };
                        items.push(Item::NoteMark(vis, kind));
                    }
                }
                0x05 if chp.special && main => {
                    if let Some(&n) = self.comment_refs.get(&(cp as u32)) {
                        items.push(Item::CommentRef(vis, n));
                    }
                }
                0x08 if chp.special => {
                    let c = self.fib.ccp;
                    let hdr = (c[0] + c[1]) as usize..(c[0] + c[1] + c[2]) as usize;
                    let spa = if main {
                        self.spas.get(&(cp as u32)).copied()
                    } else if hdr.contains(&cp) {
                        self.hdr_spas.get(&((cp - hdr.start) as u32)).copied()
                    } else {
                        None
                    };
                    match spa {
                        Some(spa) if self.in_text_box == 0 => {
                            self.in_header = !main;
                            let f = self.floating(spa);
                            self.in_header = false;
                            if let Some(f) = f {
                                items.push(Item::Float(vis, Box::new(f)));
                            }
                        }
                        _ => self.note("floating objects in text boxes and notes not imported"),
                    }
                }
                0x28 if chp.special && chp.symbol.is_some() => {
                    let (f, c) = chp.symbol.expect("checked");
                    let mut v = vis;
                    v.fonts = [Some(f), Some(f), Some(f), Some(f)];
                    if let Some(ch) = char::from_u32(c as u32) {
                        push_text(&mut items, v, &ch.to_string());
                    }
                }
                _ if u < 0x20 => {}
                _ => {
                    // UTF-16 text; keep surrogate pairs together.
                    let mut buf = vec![u];
                    if (0xD800..0xDC00).contains(&u)
                        && let Some(&lo) = self.units.get(cp + 1)
                        && (0xDC00..0xE000).contains(&lo)
                        && cp + 1 < end
                    {
                        buf.push(lo);
                        cp += 1;
                    }
                    let s: String = char::decode_utf16(buf)
                        .map(|c| c.unwrap_or('\u{FFFD}'))
                        .collect();
                    push_text(&mut items, vis, &s);
                }
            }
            cp += 1;
        }
        if !items.is_empty() {
            // Text after the last paragraph mark (a damaged story).
            out.push((
                Para {
                    pap: Pap::default(),
                    mark: Chp::default(),
                    items,
                    section_end: None,
                    nested: None,
                },
                0x0D,
            ));
        }
        out
    }
}

/// The URL of a `HYPERLINK "url"` field (not local `\l` links).
/// The strings of an extended STTB (`fExtend` = 0xFFFF, UTF-16).
fn sttb_strings(b: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    if u16_at(b, 0) != Some(0xFFFF) {
        return out;
    }
    let count = u16_at(b, 2).unwrap_or(0) as usize;
    let extra = u16_at(b, 4).unwrap_or(0) as usize;
    let mut at = 6;
    for _ in 0..count {
        let Some(cch) = u16_at(b, at) else { break };
        let units: Vec<u16> = (0..cch as usize)
            .map_while(|k| u16_at(b, at + 2 + k * 2))
            .collect();
        out.push(String::from_utf16_lossy(&units));
        at += 2 + cch as usize * 2 + extra;
    }
    out
}

/// A DTTM ([MS-DOC] 2.9.66: minutes, hours, day, month, years since
/// 1900, weekday in bit fields) as an `xsd:dateTime`; `None` when empty.
pub(crate) fn dttm(v: u32) -> Option<String> {
    let min = v & 0x3F;
    let hour = (v >> 6) & 0x1F;
    let day = (v >> 11) & 0x1F;
    let month = (v >> 16) & 0x0F;
    let year = 1900 + ((v >> 20) & 0x1FF);
    if v == 0 || day == 0 || !(1..=12).contains(&month) || hour > 23 || min > 59 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:00Z"
    ))
}

fn hyperlink_url(instr: &str) -> Option<String> {
    let rest = instr.trim_start().get(9..)?.trim();
    if rest.starts_with("\\l") {
        return None;
    }
    let url = match rest.strip_prefix('"') {
        Some(q) => q.split('"').next()?,
        None => rest.split_whitespace().next()?,
    };
    (!url.is_empty()).then(|| url.to_owned())
}

/// PNG or JPEG bytes inside a blip (after its one or two UIDs and tag).
fn sniff_image(b: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    for off in [17, 33, 0, 16, 32] {
        let s = b.get(off..)?;
        if s.starts_with(PNG) {
            return Some(("png", s.to_vec()));
        }
        if s.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(("jpeg", s.to_vec()));
        }
    }
    None
}

/// A style (STD).
fn parse_std(std: &[u8], cb_base: usize) -> Option<StyleDef> {
    let sti = u16_at(std, 0)? & 0x0FFF;
    let w2 = u16_at(std, 2)?;
    let w4 = u16_at(std, 4)?;
    let kind = (w2 & 0xF) as u8;
    let base = w2 >> 4;
    let cupx = (w4 & 0xF) as usize;
    let next = w4 >> 4;
    let cch = u16_at(std, cb_base)? as usize;
    let name: Vec<u16> = (0..cch)
        .map_while(|k| u16_at(std, cb_base + 2 + k * 2))
        .collect();
    let mut name = String::from_utf16_lossy(&name);
    // "Heading 1,h1": aliases after a comma.
    if let Some(i) = name.find(',') {
        name.truncate(i);
    }
    let mut at = cb_base + 2 + cch * 2 + 2;
    let mut upx = Vec::new();
    for _ in 0..cupx {
        if at % 2 == 1 {
            at += 1;
        }
        let Some(cb) = u16_at(std, at) else { break };
        let Some(b) = std.get(at + 2..at + 2 + cb as usize) else {
            break;
        };
        upx.push(b.to_vec());
        at += 2 + cb as usize;
    }
    let (pap, chp) = match kind {
        // Paragraph: PAPX (istd + grpprl), CHPX.
        1 => (
            upx.first()
                .and_then(|p| p.get(2..))
                .unwrap_or_default()
                .to_vec(),
            upx.get(1).cloned().unwrap_or_default(),
        ),
        2 => (Vec::new(), upx.first().cloned().unwrap_or_default()),
        // Table: TAPX, PAPX, CHPX.
        3 => (
            upx.get(1)
                .and_then(|p| p.get(2..))
                .unwrap_or_default()
                .to_vec(),
            upx.get(2).cloned().unwrap_or_default(),
        ),
        _ => (
            upx.first()
                .and_then(|p| p.get(2..))
                .unwrap_or_default()
                .to_vec(),
            Vec::new(),
        ),
    };
    Some(StyleDef {
        name,
        sti,
        kind,
        base: (base != 0xFFF).then_some(base),
        next: (next != 0xFFF).then_some(next),
        pap,
        chp,
    })
}

/// A list level (LVL) at `at`; also where the next one starts.
fn parse_lvl(s: &[u8], at: usize) -> Option<(Level, usize)> {
    let start = i32_at(s, at)?;
    let nfc = *s.get(at + 4)?;
    let flags = *s.get(at + 5)?;
    let follow = *s.get(at + 15)?;
    let cb_chp = *s.get(at + 24)? as usize;
    let cb_pap = *s.get(at + 25)? as usize;
    let mut p = at + 28;
    let pap = s.get(p..p + cb_pap)?.to_vec();
    p += cb_pap;
    let chp = s.get(p..p + cb_chp)?.to_vec();
    p += cb_chp;
    let cch = u16_at(s, p)? as usize;
    let units: Vec<u16> = (0..cch).map_while(|k| u16_at(s, p + 2 + k * 2)).collect();
    if units.len() != cch {
        return None;
    }
    p += 2 + cch * 2;
    let mut text = String::new();
    for c in char::decode_utf16(units) {
        match c {
            Ok(c) if (c as u32) < 9 => text.push_str(&format!("%{}", c as u32 + 1)),
            Ok(c) => text.push(c),
            Err(_) => text.push('\u{FFFD}'),
        }
    }
    Some((
        Level {
            start,
            nfc,
            jc: flags & 3,
            legal: flags & 4 != 0,
            follow,
            text,
            pap,
            chp,
        },
        p,
    ))
}

/// Group flat paragraphs into paragraphs and tables, tables nested in
/// cells kept nested (by their depth, `sprmPItap`; inner cells and rows end
/// with `sprmPFInnerTableCell` / `sprmPFInnerTtp`).
fn assemble(paras: Vec<(Para, u16)>, _notes: &mut BTreeSet<String>) -> Vec<Block> {
    #[derive(Default)]
    struct Level {
        rows: Vec<Row>,
        cells: Vec<Vec<Para>>,
        cell: Vec<Para>,
    }
    /// Close the innermost table: it goes into its parent's cell as one
    /// nested table (an unfinished row's text as paragraphs).
    fn close(stack: &mut Vec<Level>, out: &mut Vec<Block>) {
        let Some(l) = stack.pop() else { return };
        let leftovers: Vec<Para> = l.cells.into_iter().flatten().chain(l.cell).collect();
        match stack.last_mut() {
            Some(parent) => {
                if !l.rows.is_empty() {
                    parent.cell.push(Para {
                        pap: Pap::default(),
                        mark: Chp::default(),
                        items: Vec::new(),
                        section_end: None,
                        nested: Some(l.rows),
                    });
                }
                parent.cell.extend(leftovers);
            }
            None => {
                if !l.rows.is_empty() {
                    out.push(Block::Table(l.rows));
                }
                out.extend(leftovers.into_iter().map(|p| Block::Para(Box::new(p))));
            }
        }
    }
    let mut out = Vec::new();
    let mut stack: Vec<Level> = Vec::new();
    for (p, mark) in paras {
        let depth = p.pap.itap.unwrap_or(if p.pap.in_table { 1 } else { 0 });
        let depth = if p.pap.in_table { depth.max(1) } else { depth }.clamp(0, 16) as usize;
        if depth == 0 {
            while !stack.is_empty() {
                close(&mut stack, &mut out);
            }
            out.push(Block::Para(Box::new(p)));
            continue;
        }
        while stack.len() > depth {
            close(&mut stack, &mut out);
        }
        while stack.len() < depth {
            stack.push(Level::default());
        }
        let l = stack.last_mut().expect("a level");
        let row_end = if depth == 1 {
            p.pap.ttp
        } else {
            p.pap.inner_ttp
        };
        if row_end {
            l.cells.push(std::mem::take(&mut l.cell));
            // The row-end mark itself is no content; a cell left open
            // before it (no cell mark) is kept.
            let mut row_cells = std::mem::take(&mut l.cells);
            row_cells.retain(|c| !c.is_empty());
            l.rows.push(Row {
                tap: p.pap.tap.clone(),
                cells: row_cells,
            });
            continue;
        }
        let cell_end = if depth == 1 {
            mark == 0x07
        } else {
            p.pap.inner_cell
        };
        let mut p = p;
        p.pap.in_table = true;
        l.cell.push(p);
        if cell_end {
            l.cells.push(std::mem::take(&mut l.cell));
        }
    }
    while !stack.is_empty() {
        close(&mut stack, &mut out);
    }
    out
}
