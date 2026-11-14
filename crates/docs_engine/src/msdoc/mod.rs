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
//!   borders, shading, row heights); nested tables are flattened;
//! - sections: page size, orientation, margins, header distances, title
//!   page, page number restarts;
//! - inline PNG and JPEG pictures (PICF + OfficeArt blips in the Data
//!   stream).
//!
//! Not imported (listed in [`Imported::notes`]): floating pictures and
//! shapes, text boxes, comments, WMF/EMF pictures and embedded objects,
//! hidden text. Tracked changes are shown accepted.
//!
//! Encrypted files and files from Word 6 / Word 95 are refused with a
//! message. A damaged file is an error, never a panic: every offset is
//! checked.

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
const PLCFSED: usize = 6;
const PLCFHDD: usize = 11;
const PLCFBTE_CHPX: usize = 12;
const PLCFBTE_PAPX: usize = 13;
const STTBF_FFN: usize = 15;
const DOP: usize = 31;
const CLX: usize = 33;
const PLC_SPA_MOM: usize = 40;
const PLCFEND_REF: usize = 46;
const PLCFEND_TXT: usize = 47;
const PLF_LST: usize = 73;
const PLF_LFO: usize = 74;

impl Fib {
    fn read(w: &[u8]) -> Result<Self> {
        if u16_at(w, 0) != Some(0xA5EC) {
            bail!("it is not a Word document");
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
}

#[derive(Clone, Debug)]
pub(crate) struct Para {
    pub pap: Pap,
    /// Formatting of the paragraph mark.
    pub mark: Chp,
    pub items: Vec<Item>,
    /// Ends section `n` (not the last one).
    pub section_end: Option<usize>,
}

pub(crate) struct Row {
    pub tap: Tap,
    pub cells: Vec<Vec<Para>>,
}

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
    /// Pictures: extension (`png` / `jpeg`) and bytes.
    pub media: Vec<(&'static str, Vec<u8>)>,
    pub facing_pages: bool,
    pub default_tab: u16,
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
        if ccp[3] > 0 {
            self.note("comments not imported");
        }
        if ccp[5] > 0 || ccp[6] > 0 {
            self.note("text boxes not imported");
        }
        if self.tbl(PLC_SPA_MOM).is_some() {
            self.note("floating pictures and shapes not imported");
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

        let body = self.story(0, main_end, true);
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
            media: std::mem::take(&mut self.media),
            facing_pages,
            default_tab,
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
                    },
                    u,
                ));
                cp += 1;
                continue;
            }
            if chp.deleted {
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
                0x08 if chp.special => {
                    self.note("floating pictures and shapes not imported");
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
                },
                0x0D,
            ));
        }
        out
    }
}

/// The URL of a `HYPERLINK "url"` field (not local `\l` links).
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

/// Group flat paragraphs into paragraphs and tables. Nested tables are
/// flattened into their outer cell.
fn assemble(paras: Vec<(Para, u16)>, notes: &mut BTreeSet<String>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut cells: Vec<Vec<Para>> = Vec::new();
    let mut cell: Vec<Para> = Vec::new();
    let flush = |out: &mut Vec<Block>,
                 rows: &mut Vec<Row>,
                 cells: &mut Vec<Vec<Para>>,
                 cell: &mut Vec<Para>| {
        if !rows.is_empty() {
            out.push(Block::Table(std::mem::take(rows)));
        }
        // An unfinished row (damaged file): keep its text as paragraphs.
        for p in cells.drain(..).flatten().chain(cell.drain(..)) {
            out.push(Block::Para(Box::new(p)));
        }
    };
    for (p, mark) in paras {
        let depth = p.pap.itap.unwrap_or(if p.pap.in_table { 1 } else { 0 });
        let in_table = p.pap.in_table || depth > 0;
        if !in_table {
            flush(&mut out, &mut rows, &mut cells, &mut cell);
            out.push(Block::Para(Box::new(p)));
            continue;
        }
        if depth > 1 {
            notes.insert("nested tables flattened".into());
            if p.pap.inner_ttp {
                continue;
            }
            let mut p = p;
            p.pap.in_table = true;
            cell.push(p);
            continue;
        }
        if p.pap.ttp {
            cells.push(std::mem::take(&mut cell));
            // The row-end mark itself is no content; a cell left open
            // before it (no cell mark) is kept.
            let mut row_cells = std::mem::take(&mut cells);
            row_cells.retain(|c| !c.is_empty());
            rows.push(Row {
                tap: p.pap.tap.clone(),
                cells: row_cells,
            });
            continue;
        }
        cell.push(p);
        if mark == 0x07 {
            cells.push(std::mem::take(&mut cell));
        }
    }
    flush(&mut out, &mut rows, &mut cells, &mut cell);
    out
}
