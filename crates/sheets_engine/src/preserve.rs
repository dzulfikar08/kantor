//! Round-trip safety for `.xlsx` (plan.md §5): saving never drops what we
//! don't understand.
//!
//! IronCalc writes a fresh package from its model and leaves out every part it
//! doesn't model (cell comments, custom document properties, drawings, print
//! settings, ...). After it has written the package to memory, [`finish`]
//! post-processes the zip:
//!
//! 1. **Error values.** IronCalc writes errors such as `#ERROR!` (e.g. 3-D
//!    references it can't evaluate) as `<c t="e"><v>#ERROR!</v>`, but OOXML only
//!    knows seven error codes, and Excel/calamine reject the sheet. Formula cells
//!    lose the bad cached value (Excel recalculates them); constant cells become
//!    `#VALUE!`.
//! 2. **Carrying parts.** When the workbook came from an `.xlsx`, parts of the
//!    original that IronCalc dropped are copied over with everything they link
//!    to, and the relationship and content-type entries are added:
//!    * package level (`_rels/.rels`): e.g. `docProps/custom.xml`, thumbnails;
//!    * workbook level: parts the workbook markup doesn't point to (custom XML,
//!      threaded-comment persons, connections); `calcChain` is always dropped;
//!    * sheet level: relationships the sheet markup doesn't point to (comments,
//!      threaded comments), and a fixed set of sheet elements IronCalc doesn't
//!      write ([`CARRIED_ELEMENTS`]: data validation, print setup, drawings,
//!      `legacyDrawing` for comment boxes, ...) with the parts they point to.
//!
//!    Original sheets are matched to saved ones by `sheetId` (IronCalc keeps it;
//!    renames and reorders keep the match). Parts of a sheet that is gone are
//!    dropped. Caveat: IronCalc gives a new sheet `max(sheetId) + 1`, so
//!    deleting the *last-numbered* sheet and then adding one reuses its id and
//!    the new sheet would inherit the old one's extras; an add-sheet API in
//!    `sheets_engine` must pick ids above every id the original had.
//!    Cell-anchored content (comments, validations, drawings) is copied as is:
//!    if rows or columns were inserted or deleted (not possible in the UI yet),
//!    it drifts.
//!
//! Everything is conservative: anything ambiguous (unknown markup around a
//! reference, name collisions, broken links in the original) is skipped and
//! reported in the notes. The merged package is only used if it passes
//! [`package_problems`] (no worse than IronCalc's own output) and IronCalc can
//! read it back; otherwise IronCalc's output (with fixed error values) is saved.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read, Write};

use roxmltree::{Document, Node};

const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_R_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const NS_PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";
const CONTENT_TYPES: &str = "[Content_Types].xml";

/// The error codes OOXML defines for `<c t="e">` (ECMA-376 Part 1, 18.17.4.4).
const VALID_ERRORS: [&str; 7] = [
    "#NULL!", "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A",
];

/// Children of `<worksheet>` in schema order (CT_Worksheet).
const SHEET_ORDER: [&str; 39] = [
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetCalcPr",
    "sheetProtection",
    "protectedRanges",
    "scenarios",
    "autoFilter",
    "sortState",
    "dataConsolidate",
    "customSheetViews",
    "mergeCells",
    "phoneticPr",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "customProperties",
    "cellWatches",
    "ignoredErrors",
    "smartTags",
    "drawing",
    "legacyDrawing",
    "legacyDrawingHF",
    "drawingHF",
    "picture",
    "oleObjects",
    "controls",
    "webPublishItems",
    "tableParts",
    "extLst",
];

/// Sheet elements copied from the original when IronCalc's output lacks them.
/// All are self-contained apart from `r:id`s, which are carried with their
/// targets. `sheetPr` is carried without its `tabColor`: the tab colour is
/// written from the model ([`write_tab_colors`]). `sheetFormatPr` holds sheet
/// defaults (row height, column width) Kantor can't change. Left out on
/// purpose: what IronCalc models (views, columns, merges, conditional
/// formats, hyperlinks), `tableParts`
/// (Excel repairs a table whose header cells no longer match its column names),
/// and `oleObjects`/`controls` (built on `mc:AlternateContent`).
pub(crate) const CARRIED_ELEMENTS: [&str; 17] = [
    "sheetPr",
    "sheetFormatPr",
    "sheetProtection",
    "protectedRanges",
    "autoFilter",
    "dataValidations",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "ignoredErrors",
    "drawing",
    "legacyDrawing",
    "legacyDrawingHF",
    "picture",
];

/// Relationship types never carried, whatever level they're on: stale caches,
/// or parts that only work with markup IronCalc rewrites.
const NEVER_CARRY: [&str; 9] = [
    "calcChain",
    "worksheet",
    "chartsheet",
    "dialogsheet",
    "externalLink",
    "pivotCacheDefinition",
    "pivotTable",
    "revisionHeaders",
    "vbaProject",
];

/// Largest part read from the original (guards against zip bombs).
const MAX_PART: u64 = 512 * 1024 * 1024;

/// The bytes to save and what preservation had to report.
pub(crate) struct Finished {
    pub bytes: Vec<u8>,
    pub notes: Vec<String>,
}

/// A sheet's tab colour as the model has it, by `sheetId`: the attributes of
/// `<tabColor>` (e.g. `rgb="FF4472C4"`), `None` for no colour.
pub(crate) type TabColors = HashMap<u32, Option<String>>;

/// Post-process IronCalc's xlsx: carry over what the `original` package had
/// and IronCalc dropped, write tab colours, fix error values. Never fails: on
/// any problem the result falls back to IronCalc's own output (with tab
/// colours and error values fixed when possible).
///
/// `full_calc` asks Excel to recalculate when it opens the file
/// (`fullCalcOnLoad`), for workbooks saved with values that may be stale.
pub(crate) fn finish(
    ironcalc: Vec<u8>,
    original: Option<&[u8]>,
    tabs: &TabColors,
    full_calc: bool,
) -> Finished {
    let mut notes = Vec::new();
    let new = match Package::read(&ironcalc) {
        Ok(p) => p,
        Err(e) => {
            notes.push(format!("could not post-process the saved package: {e}"));
            return Finished {
                bytes: ironcalc,
                notes,
            };
        }
    };
    let mut base = new.clone();
    let baseline = match polish(&mut base, tabs, full_calc, &mut notes).and_then(|()| base.write())
    {
        Ok(b) => b,
        Err(e) => {
            notes.push(format!("could not post-process the saved package: {e}"));
            ironcalc
        }
    };
    let Some(original) = original else {
        return Finished {
            bytes: baseline,
            notes,
        };
    };

    let mut merged = new;
    let mut merge_notes = Vec::new();
    let bytes = match merge(&mut merged, original, &mut merge_notes) {
        Ok(false) => {
            notes.extend(merge_notes);
            return Finished {
                bytes: baseline,
                notes,
            };
        }
        Ok(true) => {
            polish(&mut merged, tabs, full_calc, &mut Vec::new()).and_then(|()| merged.write())
        }
        Err(e) => Err(e),
    };
    notes.extend(merge_notes);
    let checked = bytes.and_then(|bytes| {
        let before: HashSet<String> = package_problems(&baseline).into_iter().collect();
        let added: Vec<String> = package_problems(&bytes)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        if !added.is_empty() {
            return Err(format!("the result would be invalid: {}", added.join("; ")));
        }
        ironcalc::import::load_from_xlsx_bytes(&bytes, "check", "en", "UTC")
            .map_err(|e| format!("IronCalc can't read the result: {e}"))?;
        Ok(bytes)
    });
    match checked {
        Ok(bytes) => Finished { bytes, notes },
        Err(e) => {
            notes.push(format!("not keeping the original's extra parts: {e}"));
            Finished {
                bytes: baseline,
                notes,
            }
        }
    }
}

/// The fixes every save gets: tab colours and valid error values (and the
/// recalculation request, when asked for).
fn polish(
    pkg: &mut Package,
    tabs: &TabColors,
    full_calc: bool,
    notes: &mut Vec<String>,
) -> Result<(), String> {
    write_tab_colors(pkg, tabs)?;
    fix_error_values(pkg, notes)?;
    if full_calc {
        request_full_calc(pkg)?;
    }
    Ok(())
}

/// Ask for a full recalculation when the workbook is opened.
fn request_full_calc(pkg: &mut Package) -> Result<(), String> {
    if let Some(wb) = pkg.text("xl/workbook.xml")?
        && wb.matches("<calcPr/>").count() == 1
    {
        let wb = wb.replace("<calcPr/>", "<calcPr fullCalcOnLoad=\"1\"/>");
        pkg.set("xl/workbook.xml", wb.into_bytes());
    }
    Ok(())
}

/// Whether an `.xlsx` asks to be recalculated when opened (`fullCalcOnLoad`
/// in `xl/workbook.xml`). Reads only that part; `false` on any problem.
pub(crate) fn asks_full_calc(reader: impl Read + std::io::Seek) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(reader) else {
        return false;
    };
    let Ok(entry) = zip.by_name("xl/workbook.xml") else {
        return false;
    };
    let mut xml = String::new();
    if entry.take(MAX_PART).read_to_string(&mut xml).is_err() {
        return false;
    }
    let Some(at) = xml.find("<calcPr") else {
        return false;
    };
    let tag = &xml[at..];
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    tag.contains("fullCalcOnLoad=\"1\"") || tag.contains("fullCalcOnLoad=\"true\"")
}

// ------------------------------------------------------------------ packages

/// A zip package held in memory, entry order kept.
#[derive(Clone, Default)]
struct Package {
    /// `(name, data)`; directory entries have names ending in `/` and no data.
    entries: Vec<(String, Vec<u8>)>,
    /// Lower-cased name -> index (part names are case-insensitive).
    index: HashMap<String, usize>,
}

impl Package {
    fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut pkg = Package::default();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().map_err(|e| e.to_string())?.into_owned();
            let mut data = Vec::new();
            if !entry.is_dir() {
                (&mut entry)
                    .take(MAX_PART + 1)
                    .read_to_end(&mut data)
                    .map_err(|e| format!("{name}: {e}"))?;
                if data.len() as u64 > MAX_PART {
                    return Err(format!("{name} is too large"));
                }
            }
            pkg.set(&name, data);
        }
        Ok(pkg)
    }

    fn write(&self) -> Result<Vec<u8>, String> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &self.entries {
            if name.ends_with('/') {
                zip.add_directory(name.as_str(), options)
                    .map_err(|e| e.to_string())?;
            } else {
                zip.start_file(name.as_str(), options)
                    .map_err(|e| e.to_string())?;
                zip.write_all(data).map_err(|e| e.to_string())?;
            }
        }
        Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
    }

    fn get(&self, name: &str) -> Option<&[u8]> {
        self.index
            .get(&name.to_lowercase())
            .map(|&i| self.entries[i].1.as_slice())
    }

    fn text(&self, name: &str) -> Result<Option<&str>, String> {
        self.get(name)
            .map(|b| std::str::from_utf8(strip_bom(b)).map_err(|_| format!("{name} is not UTF-8")))
            .transpose()
    }

    fn has(&self, name: &str) -> bool {
        self.index.contains_key(&name.to_lowercase())
    }

    fn set(&mut self, name: &str, data: Vec<u8>) {
        match self.index.get(&name.to_lowercase()) {
            Some(&i) => self.entries[i].1 = data,
            None => {
                self.index.insert(name.to_lowercase(), self.entries.len());
                self.entries.push((name.to_owned(), data));
            }
        }
    }

    /// Part names (no directories, no `[Content_Types].xml`).
    fn parts(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !n.ends_with('/') && *n != CONTENT_TYPES)
    }
}

/// The original package, read lazily: only the parts the merge looks at are
/// decompressed.
struct Original<'a> {
    zip: RefCell<zip::ZipArchive<Cursor<&'a [u8]>>>,
    /// Lower-cased name -> name as stored.
    names: HashMap<String, String>,
}

impl<'a> Original<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut names = HashMap::new();
        for n in zip.file_names() {
            let n = n.map_err(|e| e.to_string())?;
            if !n.ends_with('/') {
                names.insert(n.to_lowercase(), n.into_owned());
            }
        }
        Ok(Self {
            zip: RefCell::new(zip),
            names,
        })
    }

    fn has(&self, name: &str) -> bool {
        self.names.contains_key(&name.to_lowercase())
    }

    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(stored) = self.names.get(&name.to_lowercase()) else {
            return Ok(None);
        };
        let mut zip = self.zip.borrow_mut();
        let entry = zip.by_name(stored).map_err(|e| format!("{name}: {e}"))?;
        let mut data = Vec::new();
        entry
            .take(MAX_PART + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("{name}: {e}"))?;
        if data.len() as u64 > MAX_PART {
            return Err(format!("{name} is too large"));
        }
        Ok(Some(data))
    }

    fn text(&self, name: &str) -> Result<Option<String>, String> {
        self.get(name)?
            .map(|b| {
                String::from_utf8(strip_bom(&b).to_vec())
                    .map_err(|_| format!("{name} is not UTF-8"))
            })
            .transpose()
    }
}

fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b)
}

// ------------------------------------------------------------------ paths

/// `"xl/worksheets/sheet1.xml" -> "xl/worksheets/"`, `"a.xml" -> ""`.
fn dir_of(part: &str) -> &str {
    part.rfind('/').map_or("", |i| &part[..=i])
}

/// The relationships part of `source` (`""` is the package itself).
fn rels_part(source: &str) -> String {
    if source.is_empty() {
        return "_rels/.rels".to_owned();
    }
    let dir = dir_of(source);
    format!("{dir}_rels/{}.rels", &source[dir.len()..])
}

/// The part a relationship `target` of `source` points to, or `None` if the
/// target is something we don't want to interpret (escapes, URLs, `..` above
/// the root).
fn resolve(source: &str, target: &str) -> Option<String> {
    if target.is_empty() || target.contains(['%', '?', '#', '\\', ':']) {
        return None;
    }
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_owned(),
        None => format!("{}{target}", dir_of(source)),
    };
    let mut segs: Vec<&str> = Vec::new();
    for s in joined.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            s => segs.push(s),
        }
    }
    (!segs.is_empty()).then(|| segs.join("/"))
}

/// A relative target from `source` to `part`, e.g. `xl/worksheets/sheet1.xml`
/// to `xl/comments1.xml` gives `../comments1.xml`.
fn relative(source: &str, part: &str) -> String {
    let from: Vec<&str> = dir_of(source)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to: Vec<&str> = part.split('/').collect();
    let common = from
        .iter()
        .zip(&to[..to.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = "../".repeat(from.len() - common);
    out.push_str(&to[common..].join("/"));
    out
}

// ------------------------------------------------------------------ relationships

#[derive(Clone, Debug)]
struct Rel {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

impl Rel {
    /// The last segment of the type, e.g. `comments`.
    fn short_kind(&self) -> &str {
        self.kind.rsplit('/').next().unwrap_or_default()
    }
}

fn parse_rels(xml: &str) -> Result<Vec<Rel>, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "Relationships" || root.tag_name().namespace() != Some(NS_PKG_REL)
    {
        return Err("not a relationships part".into());
    }
    let mut out = Vec::new();
    for r in root.children().filter(|n| n.is_element()) {
        if r.tag_name().name() != "Relationship" {
            return Err(format!("unexpected <{}>", r.tag_name().name()));
        }
        let get = |a: &str| r.attribute(a).map(str::to_owned);
        out.push(Rel {
            id: get("Id").ok_or("relationship without Id")?,
            kind: get("Type").ok_or("relationship without Type")?,
            target: get("Target").ok_or("relationship without Target")?,
            external: r.attribute("TargetMode") == Some("External"),
        });
    }
    Ok(out)
}

fn write_rels(rels: &[Rel]) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{NS_PKG_REL}\">"
    );
    for r in rels {
        out.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>",
            esc_attr(&r.id),
            esc_attr(&r.kind),
            esc_attr(&r.target),
            if r.external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        ));
    }
    out.push_str("</Relationships>");
    out
}

/// The next free `rIdN` ids for a relationships part.
struct IdAlloc {
    used: HashSet<String>,
    next: u32,
}

impl IdAlloc {
    fn new(rels: &[Rel]) -> Self {
        let used: HashSet<String> = rels.iter().map(|r| r.id.clone()).collect();
        let next = rels
            .iter()
            .filter_map(|r| r.id.strip_prefix("rId")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        Self { used, next }
    }

    fn next(&mut self) -> String {
        loop {
            let id = format!("rId{}", self.next);
            self.next += 1;
            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

/// Every attribute value in the relationships namespace (`r:id`, `r:embed`...)
/// anywhere under `node`.
fn rel_refs(node: Node) -> BTreeSet<String> {
    node.descendants()
        .filter(|n| n.is_element())
        .flat_map(|n| n.attributes())
        .filter(|a| matches!(a.namespace(), Some(NS_R) | Some(NS_R_STRICT)))
        .map(|a| a.value().to_owned())
        .collect()
}

// ------------------------------------------------------------------ content types

#[derive(Default)]
struct ContentTypes {
    /// Lower-cased extension -> type.
    defaults: HashMap<String, String>,
    /// Lower-cased part name without the leading `/` -> type.
    overrides: HashMap<String, String>,
}

impl ContentTypes {
    fn parse(xml: &str) -> Result<Self, String> {
        let doc = Document::parse(xml).map_err(|e| e.to_string())?;
        let mut ct = ContentTypes::default();
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            let kind = n
                .attribute("ContentType")
                .ok_or("entry without ContentType")?;
            match n.tag_name().name() {
                "Default" => {
                    let ext = n
                        .attribute("Extension")
                        .ok_or("Default without Extension")?;
                    ct.defaults.insert(ext.to_lowercase(), kind.to_owned());
                }
                "Override" => {
                    let part = n.attribute("PartName").ok_or("Override without PartName")?;
                    let part = part.strip_prefix('/').unwrap_or(part);
                    ct.overrides.insert(part.to_lowercase(), kind.to_owned());
                }
                other => return Err(format!("unexpected <{other}> in content types")),
            }
        }
        Ok(ct)
    }

    fn of(&self, part: &str) -> Option<&str> {
        let lower = part.to_lowercase();
        if let Some(t) = self.overrides.get(&lower) {
            return Some(t);
        }
        let file = &lower[dir_of(&lower).len()..];
        let ext = file.rsplit_once('.')?.1;
        self.defaults.get(ext).map(String::as_str)
    }

    fn is_default(&self, part: &str) -> bool {
        !self.overrides.contains_key(&part.to_lowercase())
    }
}

fn extension(part: &str) -> Option<String> {
    let file = &part[dir_of(part).len()..];
    file.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| !e.is_empty())
}

// ------------------------------------------------------------------ validity

/// Package-level problems of an xlsx: entries that don't unzip, parts without a
/// content type, relationships whose source or target part is missing,
/// duplicate relationship ids. An empty list means the package structure is
/// sound (it says nothing about the XML inside the parts).
pub fn package_problems(bytes: &[u8]) -> Vec<String> {
    let pkg = match Package::read(bytes) {
        Ok(p) => p,
        Err(e) => return vec![format!("not a readable zip: {e}")],
    };
    let mut out = Vec::new();
    let ct = match pkg.text(CONTENT_TYPES).and_then(|t| {
        t.ok_or_else(|| "missing [Content_Types].xml".to_owned())
            .and_then(ContentTypes::parse)
    }) {
        Ok(ct) => ct,
        Err(e) => return vec![format!("content types: {e}")],
    };
    for part in pkg.parts() {
        if ct.of(part).is_none() {
            out.push(format!("{part}: no content type"));
        }
    }
    for part in ct.overrides.keys() {
        if !pkg.has(part) {
            out.push(format!("content type override for missing part /{part}"));
        }
    }
    let rels_parts: Vec<&str> = pkg
        .parts()
        .filter(|p| p.ends_with(".rels") && dir_of(p).ends_with("_rels/"))
        .collect();
    for rp in rels_parts {
        let dir = dir_of(rp);
        let parent = &dir[..dir.len() - "_rels/".len()];
        let source = format!("{parent}{}", rp[dir.len()..].trim_end_matches(".rels"));
        let source = if rp == "_rels/.rels" { "" } else { &source };
        if !source.is_empty() && !pkg.has(source) {
            out.push(format!("{rp}: source part {source} is missing"));
        }
        let rels = match pkg.text(rp).and_then(|t| parse_rels(t.unwrap_or_default())) {
            Ok(r) => r,
            Err(e) => {
                out.push(format!("{rp}: {e}"));
                continue;
            }
        };
        let mut ids = HashSet::new();
        for r in &rels {
            if !ids.insert(&r.id) {
                out.push(format!("{rp}: duplicate id {}", r.id));
            }
            if r.external {
                continue;
            }
            match resolve(source, &r.target) {
                Some(t) if pkg.has(&t) => {}
                _ => out.push(format!("{rp}: target {} of {} is missing", r.target, r.id)),
            }
        }
    }
    out
}

// ------------------------------------------------------------------ error values

/// Replace error codes OOXML doesn't define. When a formula loses its cached
/// value, the workbook asks for a full recalculation on load.
fn fix_error_values(pkg: &mut Package, notes: &mut Vec<String>) -> Result<(), String> {
    let sheets: Vec<String> = pkg
        .parts()
        .filter(|p| {
            let p = p.to_lowercase();
            p.starts_with("xl/worksheets/") && p.ends_with(".xml") && !p.contains("_rels")
        })
        .map(str::to_owned)
        .collect();
    let mut formulas = 0;
    for part in sheets {
        let Some(xml) = pkg.text(&part)? else {
            continue;
        };
        if !has_invalid_error(xml) {
            continue;
        }
        let fixed = fix_sheet_errors(xml).map_err(|e| format!("{part}: {e}"))?;
        let count = fixed.formulas + fixed.constants;
        if count > 0 {
            notes.push(format!(
                "{part}: {count} cell(s) with an error value Excel doesn't know were saved without it"
            ));
            pkg.set(&part, fixed.xml.into_bytes());
            formulas += fixed.formulas;
        }
    }
    if formulas > 0
        && let Some(wb) = pkg.text("xl/workbook.xml")?
        && wb.matches("<calcPr/>").count() == 1
    {
        let wb = wb.replace("<calcPr/>", "<calcPr fullCalcOnLoad=\"1\"/>");
        pkg.set("xl/workbook.xml", wb.into_bytes());
    }
    Ok(())
}

struct FixedErrors {
    xml: String,
    /// Formula cells whose cached value was removed.
    formulas: usize,
    /// Constant cells changed to `#VALUE!`.
    constants: usize,
}

/// Cheap scan for `<v>#...</v>` values outside [`VALID_ERRORS`].
fn has_invalid_error(xml: &str) -> bool {
    xml.match_indices("<v>#").any(|(i, _)| {
        let rest = &xml[i + 3..];
        let end = rest.find('<').unwrap_or(rest.len());
        !VALID_ERRORS.contains(&&rest[..end])
    })
}

fn fix_sheet_errors(xml: &str) -> Result<FixedErrors, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut formulas = 0;
    for c in doc.descendants().filter(|n| is_main(*n, "c")) {
        if c.attribute("t") != Some("e") {
            continue;
        }
        let Some(v) = c.children().find(|n| is_main(*n, "v")) else {
            continue;
        };
        let value = v.text().unwrap_or_default().trim();
        if VALID_ERRORS.contains(&value) {
            continue;
        }
        let has_formula = c.children().any(|n| is_main(n, "f"));
        formulas += usize::from(has_formula);
        let mut out = String::from("<c");
        for a in c.attributes() {
            if a.namespace().is_none() && a.name() == "t" && has_formula {
                continue;
            }
            write_attr(&mut out, a, &HashMap::new())?;
        }
        out.push('>');
        for child in c.children() {
            if child.is_element() && is_main(child, "v") {
                if !has_formula {
                    out.push_str("<v>#VALUE!</v>");
                }
                continue;
            }
            write_node(child, &mut out, &HashMap::new())?;
        }
        out.push_str("</c>");
        edits.push((c.range(), out));
    }
    let constants = edits.len() - formulas;
    Ok(FixedErrors {
        xml: apply_edits(xml, edits),
        formulas,
        constants,
    })
}

fn apply_edits(xml: &str, mut edits: Vec<(std::ops::Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    out
}

fn is_main(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_MAIN)
}

// ------------------------------------------------------------------ tab colours

/// Set each sheet's `<sheetPr><tabColor>` from `tabs` (IronCalc 0.9.1 reads
/// tab colours but never writes them).
fn write_tab_colors(pkg: &mut Package, tabs: &TabColors) -> Result<(), String> {
    if tabs.values().all(Option::is_none)
        && !pkg.entries.iter().any(|(_, d)| contains(d, b"tabColor"))
    {
        return Ok(());
    }
    let root_rels = parse_rels(pkg.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let read = |p: &str| Ok(pkg.text(p)?.map(str::to_owned));
    let (_, _, _, sheets) = workbook_sheets(&root_rels, &read)?;
    for sheet in sheets {
        let Some(color) = tabs.get(&sheet.sheet_id) else {
            continue;
        };
        let xml = pkg.text(&sheet.part)?.ok_or("sheet is missing")?;
        if color.is_none() && !xml.contains("tabColor") {
            continue;
        }
        if let Some(fixed) = set_tab_color(xml, color.as_deref())
            .map_err(|e| format!("tab colour of {:?}: {e}", sheet.name))?
        {
            pkg.set(&sheet.part, fixed.into_bytes());
        }
    }
    Ok(())
}

/// The sheet XML with its tab colour set to `color` (attributes of
/// `<tabColor>`), or `None` if nothing changes.
fn set_tab_color(xml: &str, color: Option<&str>) -> Result<Option<String>, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.lookup_namespace_uri(None) != Some(NS_MAIN) {
        return Err("unexpected namespaces".into());
    }
    let tab = color.map(|c| format!("<tabColor {c}/>"));
    let sheet_pr = root.children().find(|n| is_main(*n, "sheetPr"));
    let edit = match sheet_pr {
        Some(pr) => {
            let mut out = String::from("<sheetPr");
            for a in pr.attributes() {
                write_attr(&mut out, a, &HashMap::new())?;
            }
            out.push('>');
            out.push_str(tab.as_deref().unwrap_or_default());
            for child in pr.children().filter(|c| !is_main(*c, "tabColor")) {
                write_node(child, &mut out, &HashMap::new())?;
            }
            out.push_str("</sheetPr>");
            (pr.range(), out)
        }
        None => {
            let Some(tab) = tab else {
                return Ok(None);
            };
            let Some(first) = root.children().find(|n| n.is_element()) else {
                return Err("empty worksheet".into());
            };
            let at = first.range().start;
            (at..at, format!("<sheetPr>{tab}</sheetPr>"))
        }
    };
    if xml[edit.0.clone()] == edit.1 {
        return Ok(None);
    }
    Ok(Some(apply_edits(xml, vec![edit])))
}

// ------------------------------------------------------------------ serializing

/// Write `node` for a document whose default namespace is SpreadsheetML and
/// that binds `r:` to the relationships namespace (IronCalc's sheets).
/// Relationship ids are renamed through `ids`. Attributes in other namespaces
/// (`xr:uid` and friends) are dropped; elements in other namespaces are an
/// error, so the caller skips the whole element rather than half of it.
fn write_node(node: Node, out: &mut String, ids: &HashMap<String, String>) -> Result<(), String> {
    write_node_without(node, out, ids, None)
}

/// [`write_node`], leaving out the children of `node` named `skip`.
fn write_node_without(
    node: Node,
    out: &mut String,
    ids: &HashMap<String, String>,
    skip: Option<&str>,
) -> Result<(), String> {
    if node.is_text() {
        out.push_str(&esc_text(node.text().unwrap_or_default()));
        return Ok(());
    }
    if !node.is_element() {
        return Ok(());
    }
    let name = node.tag_name();
    if name.namespace() != Some(NS_MAIN) {
        return Err(format!("unsupported element <{}>", name.name()));
    }
    out.push('<');
    out.push_str(name.name());
    for a in node.attributes() {
        write_attr(out, a, ids)?;
    }
    let children: Vec<Node> = node
        .children()
        .filter(|c| skip.is_none_or(|name| !is_main(*c, name)))
        .collect();
    if children.is_empty() {
        out.push_str("/>");
        return Ok(());
    }
    out.push('>');
    for child in children {
        write_node(child, out, ids)?;
    }
    out.push_str("</");
    out.push_str(name.name());
    out.push('>');
    Ok(())
}

fn write_attr(
    out: &mut String,
    a: roxmltree::Attribute,
    ids: &HashMap<String, String>,
) -> Result<(), String> {
    let (prefix, value) = match a.namespace() {
        None => ("", a.value()),
        Some(NS_XML) => ("xml:", a.value()),
        Some(NS_R) | Some(NS_R_STRICT) => (
            "r:",
            ids.get(a.value())
                .map(String::as_str)
                .ok_or_else(|| format!("unmapped relationship id {}", a.value()))?,
        ),
        Some(_) => return Ok(()),
    };
    out.push_str(&format!(" {prefix}{}=\"{}\"", a.name(), esc_attr(value)));
    Ok(())
}

fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn esc_attr(s: &str) -> String {
    esc_text(s)
        .replace('"', "&quot;")
        .replace('\t', "&#9;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
}

// ------------------------------------------------------------------ merging

struct SheetEntry {
    name: String,
    sheet_id: u32,
    part: String,
}

/// The workbook part (from `_rels/.rels`) and its worksheets in tab order.
/// Chart sheets and sheets whose part can't be resolved are left out.
fn workbook_sheets(
    root_rels: &[Rel],
    read: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Result<(String, Vec<Rel>, String, Vec<SheetEntry>), String> {
    let wb = root_rels
        .iter()
        .find(|r| r.short_kind() == "officeDocument" && !r.external)
        .and_then(|r| resolve("", &r.target))
        .ok_or("no workbook part")?;
    let wb_xml = read(&wb)?.ok_or("workbook part is missing")?;
    let wb_rels = match read(&rels_part(&wb))? {
        Some(x) => parse_rels(&x)?,
        None => Vec::new(),
    };
    let doc = Document::parse(&wb_xml).map_err(|e| format!("{wb}: {e}"))?;
    let mut sheets = Vec::new();
    for s in doc.descendants().filter(|n| is_main(*n, "sheet")) {
        let rid = s
            .attributes()
            .find(|a| matches!(a.namespace(), Some(NS_R) | Some(NS_R_STRICT)) && a.name() == "id")
            .map(|a| a.value());
        let rel = rid.and_then(|id| wb_rels.iter().find(|r| r.id == id));
        let Some(rel) = rel.filter(|r| r.short_kind() == "worksheet" && !r.external) else {
            continue;
        };
        let (Some(name), Some(id), Some(part)) = (
            s.attribute("name"),
            s.attribute("sheetId").and_then(|v| v.parse().ok()),
            resolve(&wb, &rel.target),
        ) else {
            continue;
        };
        sheets.push(SheetEntry {
            name: name.to_owned(),
            sheet_id: id,
            part,
        });
    }
    Ok((wb, wb_rels, wb_xml, sheets))
}

/// What gets added to the new package.
struct Merge<'o, 'a> {
    orig: &'o Original<'a>,
    orig_ct: ContentTypes,
    new_ct: ContentTypes,
    /// Original part names to copy (including their own `.rels` parts).
    copy: BTreeSet<String>,
    /// Lower-cased names of parts IronCalc wrote.
    written: HashSet<String>,
}

impl Merge<'_, '_> {
    /// Plan copying `part` and everything it links to. Nothing is planned if
    /// any of it can't be carried.
    fn carry(&mut self, part: &str) -> Result<(), String> {
        let mut plan = self.copy.clone();
        self.carry_into(part, &mut plan)?;
        self.copy = plan;
        Ok(())
    }

    fn carry_into(&self, part: &str, plan: &mut BTreeSet<String>) -> Result<(), String> {
        if plan.contains(part) {
            return Ok(());
        }
        if self.written.contains(&part.to_lowercase()) {
            return Err(format!("{part} is a part Kantor writes itself"));
        }
        if !self.orig.has(part) {
            return Err(format!("{part} is missing in the original"));
        }
        if self.orig_ct.of(part).is_none() {
            return Err(format!("{part} has no content type in the original"));
        }
        plan.insert(part.to_owned());
        let rp = rels_part(part);
        let Some(xml) = self.orig.text(&rp)? else {
            return Ok(());
        };
        if self.written.contains(&rp.to_lowercase()) {
            return Err(format!("{rp} is a part Kantor writes itself"));
        }
        plan.insert(rp.clone());
        for rel in parse_rels(&xml).map_err(|e| format!("{rp}: {e}"))? {
            if rel.external {
                continue;
            }
            if NEVER_CARRY.contains(&rel.short_kind()) {
                return Err(format!("{part} links to a {}", rel.short_kind()));
            }
            let target = resolve(part, &rel.target)
                .ok_or_else(|| format!("{rp}: can't follow target {}", rel.target))?;
            if target.starts_with("xl/drawings/") {
                // Slicers are anchored in drawings but defined through markup
                // IronCalc rewrites.
                if let Some(bytes) = self.orig.get(&target)?
                    && contains(&bytes, b"slicer")
                {
                    return Err(format!("{target} holds a slicer"));
                }
            }
            self.carry_into(&target, plan)?;
        }
        Ok(())
    }

    /// Copy the planned parts and register their content types.
    fn apply(&self, new: &mut Package) -> Result<(), String> {
        let mut ct_entries = String::new();
        let mut added_defaults: HashSet<String> = HashSet::new();
        for part in &self.copy {
            let data = self
                .orig
                .get(part)?
                .ok_or_else(|| format!("{part} vanished"))?;
            let kind = self
                .orig_ct
                .of(part)
                .ok_or_else(|| format!("{part} has no content type"))?;
            new.set(part, data);
            if self.new_ct.of(part) == Some(kind) {
                continue;
            }
            // A Default the original used carries over as a Default (one per
            // extension, so the type is the same for every part that uses it).
            match extension(part) {
                Some(ext)
                    if self.orig_ct.is_default(part)
                        && !self.new_ct.defaults.contains_key(&ext) =>
                {
                    if !added_defaults.contains(&ext) {
                        ct_entries.push_str(&format!(
                            "<Default Extension=\"{}\" ContentType=\"{}\"/>",
                            esc_attr(&ext),
                            esc_attr(kind)
                        ));
                        added_defaults.insert(ext);
                    }
                }
                _ => ct_entries.push_str(&format!(
                    "<Override PartName=\"/{}\" ContentType=\"{}\"/>",
                    esc_attr(part),
                    esc_attr(kind)
                )),
            }
        }
        if !ct_entries.is_empty() {
            let xml = new
                .text(CONTENT_TYPES)?
                .ok_or("missing content types")?
                .to_owned();
            let at = xml.rfind("</Types>").ok_or("content types not closed")?;
            let mut xml = xml;
            xml.insert_str(at, &ct_entries);
            new.set(CONTENT_TYPES, xml.into_bytes());
        }
        Ok(())
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Relationships to add to one rels part of the new package.
struct RelAdds {
    source: String,
    rels: Vec<Rel>,
}

/// Carry what IronCalc dropped from `original` into `new`. Returns whether
/// anything was added.
fn merge(new: &mut Package, original: &[u8], notes: &mut Vec<String>) -> Result<bool, String> {
    let orig = Original::new(original)?;
    let orig_ct = ContentTypes::parse(
        &orig
            .text(CONTENT_TYPES)?
            .ok_or("original has no content types")?,
    )?;
    let new_ct = ContentTypes::parse(new.text(CONTENT_TYPES)?.ok_or("no content types")?)?;
    let written: HashSet<String> = new.parts().map(str::to_lowercase).collect();
    let mut m = Merge {
        orig: &orig,
        orig_ct,
        new_ct,
        copy: BTreeSet::new(),
        written,
    };
    let mut adds: Vec<RelAdds> = Vec::new();
    let mut sheet_edits: Vec<(String, Vec<Insert>)> = Vec::new();

    // Package level.
    let orig_root = parse_rels(
        &orig
            .text("_rels/.rels")?
            .ok_or("original has no _rels/.rels")?,
    )?;
    let new_root = parse_rels(new.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let none = BTreeSet::new();
    adds.push(carry_implicit(
        &mut m,
        Level {
            label: "package",
            orig_source: "",
            new_source: "",
            orig_rels: &orig_root,
            new_rels: &new_root,
            explicit: &none,
            quiet: &none,
        },
        notes,
    ));

    // Workbook level.
    let orig_read = |p: &str| orig.text(p);
    let (orig_wb, orig_wb_rels, orig_wb_xml, orig_sheets) =
        workbook_sheets(&orig_root, &orig_read)?;
    let new_read = |p: &str| Ok(new.text(p)?.map(str::to_owned));
    let (new_wb, new_wb_rels, _, new_sheets) = workbook_sheets(&new_root, &new_read)?;
    let wb_refs = Document::parse(&orig_wb_xml)
        .map(|d| rel_refs(d.root()))
        .map_err(|e| format!("{orig_wb}: {e}"))?;
    adds.push(carry_implicit(
        &mut m,
        Level {
            label: "workbook",
            orig_source: &orig_wb,
            new_source: &new_wb,
            orig_rels: &orig_wb_rels,
            new_rels: &new_wb_rels,
            explicit: &wb_refs,
            quiet: &none,
        },
        notes,
    ));

    // Sheet level, matched by sheetId.
    let mut ids_seen = HashSet::new();
    let unique_ids = orig_sheets.iter().all(|s| ids_seen.insert(s.sheet_id));
    for os in &orig_sheets {
        let Some(ns) = new_sheets.iter().find(|n| n.sheet_id == os.sheet_id) else {
            if orig.has(&rels_part(&os.part)) {
                notes.push(format!(
                    "sheet {:?} was deleted: its comments, drawings and other extras are dropped",
                    os.name
                ));
            }
            continue;
        };
        if !unique_ids {
            notes.push(format!(
                "sheet {:?}: the original has duplicate sheet ids, its extras are dropped",
                os.name
            ));
            continue;
        }
        if let Some((add, edit)) = carry_sheet(&mut m, new, os, ns, notes)? {
            adds.push(add);
            if !edit.is_empty() {
                sheet_edits.push((ns.part.clone(), edit));
            }
        }
    }

    if m.copy.is_empty() && adds.iter().all(|a| a.rels.is_empty()) && sheet_edits.is_empty() {
        return Ok(false);
    }
    m.apply(new)?;
    for add in adds.into_iter().filter(|a| !a.rels.is_empty()) {
        let rp = rels_part(&add.source);
        let mut rels = match new.text(&rp)? {
            Some(x) => parse_rels(x)?,
            None => Vec::new(),
        };
        rels.extend(add.rels);
        new.set(&rp, write_rels(&rels).into_bytes());
    }
    for (part, edits) in sheet_edits {
        let xml = new.text(&part)?.ok_or("sheet vanished")?;
        let mut out = String::with_capacity(xml.len() + 1024);
        let mut last = 0;
        let mut edits = edits;
        edits.sort_by_key(|(at, order, _)| (*at, *order));
        for (at, _, text) in edits {
            out.push_str(&xml[last..at]);
            out.push_str(&text);
            last = at;
        }
        out.push_str(&xml[last..]);
        new.set(&part, out.into_bytes());
    }
    Ok(true)
}

/// One source part's relationships, original and saved.
struct Level<'r> {
    /// For notes: "package", "workbook", "sheet \"Notes\"".
    label: &'r str,
    orig_source: &'r str,
    new_source: &'r str,
    orig_rels: &'r [Rel],
    new_rels: &'r [Rel],
    /// Ids the original source's markup references.
    explicit: &'r BTreeSet<String>,
    /// Explicit ids not to report as dropped (handled elsewhere).
    quiet: &'r BTreeSet<String>,
}

/// Carry relationships of `orig_source` that its markup doesn't reference
/// (listed in `explicit`), whose type the new package doesn't already have.
fn carry_implicit(m: &mut Merge, level: Level, notes: &mut Vec<String>) -> RelAdds {
    let Level {
        label,
        orig_source,
        new_source,
        orig_rels,
        new_rels,
        explicit,
        quiet,
    } = level;
    let mut ids = IdAlloc::new(new_rels);
    let mut out = Vec::new();
    for r in orig_rels {
        let kind = r.short_kind();
        if new_rels.iter().any(|n| n.kind == r.kind) || matches!(kind, "calcChain" | "hyperlink") {
            continue;
        }
        if explicit.contains(&r.id) || NEVER_CARRY.contains(&kind) {
            if !quiet.contains(&r.id) {
                notes.push(format!("{label}: {kind} ({}) is not kept", r.target));
            }
            continue;
        }
        if r.external {
            out.push(Rel {
                id: ids.next(),
                ..r.clone()
            });
            continue;
        }
        let Some(target) = resolve(orig_source, &r.target) else {
            notes.push(format!("{label}: can't follow {kind} target {}", r.target));
            continue;
        };
        let new_target = relative(new_source, &target);
        if kind == "comments" && !new_target.starts_with("../") {
            // IronCalc's reader assumes comments live in `xl/`.
            notes.push(format!("{label}: comments at {target} are not kept"));
            continue;
        }
        match m.carry(&target) {
            Ok(()) => out.push(Rel {
                id: ids.next(),
                kind: r.kind.clone(),
                target: new_target,
                external: false,
            }),
            Err(e) => notes.push(format!("{label}: {kind} not kept: {e}")),
        }
    }
    RelAdds {
        source: new_source.to_owned(),
        rels: out,
    }
}

/// An insertion into a sheet's XML: `(byte offset, schema index, text)`.
type Insert = (usize, usize, String);

/// Relationships to add, and insertions into the sheet XML.
type SheetCarry = (RelAdds, Vec<Insert>);

/// Carry one sheet's relationships and elements.
fn carry_sheet(
    m: &mut Merge,
    new: &Package,
    os: &SheetEntry,
    ns: &SheetEntry,
    notes: &mut Vec<String>,
) -> Result<Option<SheetCarry>, String> {
    let orig_rels = match m.orig.text(&rels_part(&os.part))? {
        Some(x) => parse_rels(&x).map_err(|e| format!("{}: {e}", os.part))?,
        None => Vec::new(),
    };
    let Some(orig_xml) = m.orig.text(&os.part)? else {
        return Ok(None);
    };
    let might_carry = CARRIED_ELEMENTS.iter().any(|e| orig_xml.contains(e));
    if orig_rels.is_empty() && !might_carry {
        return Ok(None);
    }
    let orig_doc = Document::parse(&orig_xml).map_err(|e| format!("sheet {:?}: {e}", os.name))?;
    let orig_root = orig_doc.root_element();
    let new_rels = match new.text(&rels_part(&ns.part))? {
        Some(x) => parse_rels(x)?,
        None => Vec::new(),
    };
    let explicit = rel_refs(orig_root);
    let label = format!("sheet {:?}", os.name);

    // Relationships the markup doesn't point to (comments, threaded comments).
    // Explicit ones are handled with their elements below; report only those
    // the elements don't account for.
    let element_refs: BTreeSet<String> = orig_root
        .children()
        .filter(|c| {
            c.tag_name().namespace() == Some(NS_MAIN)
                && CARRIED_ELEMENTS.contains(&c.tag_name().name())
        })
        .flat_map(rel_refs)
        .collect();
    let mut adds = carry_implicit(
        m,
        Level {
            label: &label,
            orig_source: &os.part,
            new_source: &ns.part,
            orig_rels: &orig_rels,
            new_rels: &new_rels,
            explicit: &explicit,
            quiet: &element_refs,
        },
        notes,
    );

    // Elements IronCalc doesn't write, with the relationships they use.
    let new_xml = new.text(&ns.part)?.ok_or("saved sheet is missing")?;
    let new_doc = Document::parse(new_xml).map_err(|e| format!("saved {label}: {e}"))?;
    let new_root = new_doc.root_element();
    let mut present: Vec<(usize, usize)> = Vec::new(); // (schema index, start offset)
    for child in new_root.children().filter(|n| n.is_element()) {
        let Some(i) = SHEET_ORDER.iter().position(|e| is_main(child, e)) else {
            notes.push(format!(
                "{label}: saved sheet has unexpected <{}>, sheet extras not kept",
                child.tag_name().name()
            ));
            return Ok(Some((adds, Vec::new())));
        };
        present.push((i, child.range().start));
    }
    let close = new_xml[..new_root.range().end]
        .rfind("</")
        .filter(|&i| i > new_root.range().start)
        .ok_or("saved sheet is not closed")?;

    let mut ids = IdAlloc::new(&[new_rels.as_slice(), adds.rels.as_slice()].concat());
    let bindings_ok = new_root.lookup_namespace_uri(None) == Some(NS_MAIN)
        && new_root.lookup_namespace_uri(Some("r")) == Some(NS_R);
    let mut inserts: Vec<Insert> = Vec::new();
    for child in orig_root.children().filter(|n| n.is_element()) {
        let local = child.tag_name().name();
        if child.tag_name().namespace() != Some(NS_MAIN) || !CARRIED_ELEMENTS.contains(&local) {
            continue;
        }
        let order = SHEET_ORDER
            .iter()
            .position(|e| *e == local)
            .expect("carried elements are in the schema order");
        if present.iter().any(|(i, _)| *i == order) {
            continue;
        }
        // Plan the relationships this element uses.
        let mut map = HashMap::new();
        let mut rels = Vec::new();
        let saved_copy = m.copy.clone();
        let mut failed = None;
        for id in rel_refs(child) {
            let Some(r) = orig_rels.iter().find(|r| r.id == id) else {
                failed = Some(format!("relationship {id} is missing"));
                break;
            };
            if NEVER_CARRY.contains(&r.short_kind()) || r.short_kind() == "hyperlink" {
                failed = Some(format!("uses a {}", r.short_kind()));
                break;
            }
            let new_id = ids.next();
            if r.external {
                rels.push(Rel {
                    id: new_id.clone(),
                    ..r.clone()
                });
            } else {
                let target = match resolve(&os.part, &r.target) {
                    Some(t) => t,
                    None => {
                        failed = Some(format!("can't follow target {}", r.target));
                        break;
                    }
                };
                if let Err(e) = m.carry(&target) {
                    failed = Some(e);
                    break;
                }
                rels.push(Rel {
                    id: new_id.clone(),
                    kind: r.kind.clone(),
                    target: relative(&ns.part, &target),
                    external: false,
                });
            }
            map.insert(id, new_id);
        }
        let mut text = String::new();
        if failed.is_none() && !bindings_ok {
            failed = Some("saved sheet uses other namespace prefixes".into());
        }
        let skip = (local == "sheetPr").then_some("tabColor");
        if failed.is_none()
            && let Err(e) = write_node_without(child, &mut text, &map, skip)
        {
            failed = Some(e);
        }
        if let Some(why) = failed {
            m.copy = saved_copy;
            notes.push(format!("{label}: <{local}> not kept: {why}"));
            continue;
        }
        let at = present
            .iter()
            .filter(|(i, _)| *i > order)
            .map(|(_, at)| *at)
            .min()
            .unwrap_or(close);
        inserts.push((at, order, text));
        present.push((order, at));
        adds.rels.extend(rels);
    }
    Ok(Some((adds, inserts)))
}

/// The parts of a package by name (for tests).
#[cfg(test)]
pub(crate) fn parts_of(bytes: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
    let pkg = Package::read(bytes).expect("zip");
    pkg.entries
        .into_iter()
        .filter(|(n, _)| !n.ends_with('/'))
        .collect()
}

/// A two-sheet package with the extras the preservation layer carries:
/// comments (+ VML), a data validation, print setup with printer settings, a
/// drawing with a chart and an image, custom properties, a custom XML item and
/// a calcChain. `extra_sheet1` is appended to sheet 1 before `</worksheet>`.
#[cfg(test)]
pub(crate) fn rich_fixture(extra_sheet1: &str) -> Vec<u8> {
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let ws = |body: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:xr=\"http://schemas.microsoft.com/office/spreadsheetml/2014/revision\" mc:Ignorable=\"xr\" xr:uid=\"{{00000000-0001-0000-0000-000000000000}}\">{body}</worksheet>"
        )
    };
    let rels = |items: &[(&str, &str, &str)]| {
        let mut out = format!("<Relationships xmlns=\"{NS_PKG_REL}\">");
        for (id, kind, target) in items {
            out.push_str(&format!(
                "<Relationship Id=\"{id}\" Type=\"{kind}\" Target=\"{target}\"/>"
            ));
        }
        out + "</Relationships>"
    };
    let comments = |cell: &str, text: &str| {
        format!(
            "<comments xmlns=\"{NS_MAIN}\"><authors><author>Ann</author></authors><commentList><comment ref=\"{cell}\" authorId=\"0\"><text><r><t>{text}</t></r></text></comment></commentList></comments>"
        )
    };
    let vml = "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\"><v:shape id=\"_x0000_s1025\"/></xml>";
    let parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", String::from(
            "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"vml\" ContentType=\"application/vnd.openxmlformats-officedocument.vmlDrawing\"/>\
<Default Extension=\"bin\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
<Override PartName=\"/xl/comments1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml\"/>\
<Override PartName=\"/xl/comments2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml\"/>\
<Override PartName=\"/xl/drawings/drawing1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>\
<Override PartName=\"/xl/charts/chart1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>\
<Override PartName=\"/xl/calcChain.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml\"/>\
<Override PartName=\"/customXml/itemProps1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.customXmlProperties+xml\"/>\
<Override PartName=\"/docProps/custom.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.custom-properties+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
</Types>")),
        ("_rels/.rels", rels(&[
            ("rId1", &format!("{R}/officeDocument"), "xl/workbook.xml"),
            ("rId2", &format!("{R}/custom-properties"), "docProps/custom.xml"),
            ("rId3", "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties", "docProps/core.xml"),
            ("rId4", &format!("{R}/extended-properties"), "docProps/app.xml"),
        ])),
        ("docProps/core.xml", "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:creator>t</dc:creator></cp:coreProperties>".into()),
        ("docProps/app.xml", "<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>t</Application></Properties>".into()),
        ("docProps/custom.xml", "<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><property fmtid=\"{D5CDD505-2E9C-101B-9397-08002B2CF9AE}\" pid=\"2\" name=\"Dept\"><vt:lpwstr>Finance</vt:lpwstr></property></Properties>".into()),
        ("xl/workbook.xml", format!("<workbook xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Other\" sheetId=\"2\" r:id=\"rId2\"/></sheets></workbook>")),
        ("xl/_rels/workbook.xml.rels", rels(&[
            ("rId1", &format!("{R}/worksheet"), "worksheets/sheet1.xml"),
            ("rId2", &format!("{R}/worksheet"), "worksheets/sheet2.xml"),
            ("rId3", &format!("{R}/styles"), "styles.xml"),
            ("rId4", &format!("{R}/calcChain"), "calcChain.xml"),
            ("rId5", &format!("{R}/customXml"), "../customXml/item1.xml"),
        ])),
        ("xl/styles.xml", format!("<styleSheet xmlns=\"{NS_MAIN}\"><fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>")),
        ("xl/calcChain.xml", format!("<calcChain xmlns=\"{NS_MAIN}\"><c r=\"B1\" i=\"1\"/></calcChain>")),
        ("customXml/item1.xml", "<root>item</root>".into()),
        ("customXml/_rels/item1.xml.rels", rels(&[("rId1", &format!("{R}/customXmlProps"), "itemProps1.xml")])),
        ("customXml/itemProps1.xml", "<ds:datastoreItem xmlns:ds=\"http://schemas.openxmlformats.org/officeDocument/2006/customXml\" ds:itemID=\"{X}\"/>".into()),
        ("xl/worksheets/sheet1.xml", ws(&format!("<sheetPr codeName=\"Sheet1\"><tabColor rgb=\"FF00FF00\"/><pageSetUpPr fitToPage=\"1\"/></sheetPr><dimension ref=\"A1:B1\"/><sheetFormatPr defaultRowHeight=\"15\"/><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c><c r=\"B1\"><f>A1*2</f><v>2</v></c></row></sheetData><dataValidations count=\"1\"><dataValidation type=\"list\" sqref=\"A1\" xr:uid=\"{{1}}\"><formula1>\"a,b\"</formula1></dataValidation></dataValidations><pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/><pageSetup orientation=\"landscape\" r:id=\"rId1\"/><drawing r:id=\"rId2\"/><legacyDrawing r:id=\"rId3\"/>{extra_sheet1}"))),
        ("xl/worksheets/_rels/sheet1.xml.rels", rels(&[
            ("rId1", &format!("{R}/printerSettings"), "../printerSettings/printerSettings1.bin"),
            ("rId2", &format!("{R}/drawing"), "../drawings/drawing1.xml"),
            ("rId3", &format!("{R}/vmlDrawing"), "../drawings/vmlDrawing1.vml"),
            ("rId4", &format!("{R}/comments"), "../comments1.xml"),
        ])),
        ("xl/worksheets/sheet2.xml", ws("<sheetData><row r=\"1\"><c r=\"A1\"><v>5</v></c></row></sheetData><legacyDrawing r:id=\"rId1\"/>")),
        ("xl/worksheets/_rels/sheet2.xml.rels", rels(&[
            ("rId1", &format!("{R}/vmlDrawing"), "/xl/drawings/vmlDrawing2.vml"),
            ("rId2", &format!("{R}/comments"), "../comments2.xml"),
        ])),
        ("xl/comments1.xml", comments("A1", "first")),
        ("xl/comments2.xml", comments("A1", "second")),
        ("xl/drawings/vmlDrawing1.vml", vml.into()),
        ("xl/drawings/vmlDrawing2.vml", vml.into()),
        ("xl/printerSettings/printerSettings1.bin", "\u{1}\u{2}".into()),
        ("xl/drawings/drawing1.xml", "<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\"/>".into()),
        ("xl/drawings/_rels/drawing1.xml.rels", rels(&[
            ("rId1", &format!("{R}/chart"), "../charts/chart1.xml"),
            ("rId2", &format!("{R}/image"), "../media/image1.png"),
        ])),
        ("xl/charts/chart1.xml", "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"/>".into()),
        ("xl/media/image1.png", "\u{89}PNG".into()),
    ];
    let mut pkg = Package::default();
    for (name, data) in parts {
        pkg.set(name, data.into_bytes());
    }
    pkg.write().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironcalc::export::save_xlsx_to_writer;
    use ironcalc::import::load_from_xlsx_bytes;
    use ironcalc_base::Model;

    fn model(bytes: &[u8]) -> Model<'static> {
        let wb = load_from_xlsx_bytes(bytes, "t", "en", "UTC").unwrap();
        let mut m = Model::from_workbook(wb, "en").unwrap();
        m.evaluate();
        m
    }

    fn save(m: &Model, original: Option<&[u8]>) -> Finished {
        let out = save_xlsx_to_writer(m, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();
        let tabs = m
            .workbook
            .worksheets
            .iter()
            .map(|ws| (ws.sheet_id, None))
            .collect();
        let done = finish(out, original, &tabs, false);
        assert_eq!(package_problems(&done.bytes), Vec::<String>::new());
        done
    }

    fn text(parts: &std::collections::BTreeMap<String, Vec<u8>>, name: &str) -> String {
        String::from_utf8(
            parts
                .get(name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .clone(),
        )
        .unwrap()
    }

    /// The part a sheet's relationship of `kind` points to.
    fn sheet_target(
        parts: &std::collections::BTreeMap<String, Vec<u8>>,
        sheet: &str,
        kind: &str,
    ) -> Option<String> {
        let rels = parse_rels(&text(parts, &rels_part(sheet))).ok()?;
        rels.iter()
            .find(|r| r.short_kind() == kind)
            .and_then(|r| resolve(sheet, &r.target))
    }

    #[test]
    fn extras_survive_a_save() {
        let original = rich_fixture("");
        assert_eq!(package_problems(&original), Vec::<String>::new());
        let mut m = model(&original);
        m.set_user_input(0, 1, 1, "42".into()).unwrap();
        let done = save(&m, Some(&original));
        assert_eq!(done.notes, Vec::<String>::new());
        let parts = parts_of(&done.bytes);
        for p in [
            "docProps/custom.xml",
            "xl/comments1.xml",
            "xl/comments2.xml",
            "xl/drawings/vmlDrawing1.vml",
            "xl/drawings/vmlDrawing2.vml",
            "xl/printerSettings/printerSettings1.bin",
            "xl/drawings/drawing1.xml",
            "xl/charts/chart1.xml",
            "xl/media/image1.png",
            "customXml/item1.xml",
            "customXml/itemProps1.xml",
        ] {
            assert!(parts.contains_key(p), "{p} dropped");
        }
        assert!(!parts.contains_key("xl/calcChain.xml"));
        let s1 = text(&parts, "xl/worksheets/sheet1.xml");
        for e in [
            "<sheetPr codeName=\"Sheet1\"><pageSetUpPr fitToPage=\"1\"/></sheetPr>",
            "<sheetFormatPr defaultRowHeight=\"15\"/>",
            "<dataValidations count=\"1\"><dataValidation type=\"list\" sqref=\"A1\"><formula1>\"a,b\"</formula1></dataValidation></dataValidations>",
            "<pageSetup orientation=\"landscape\" r:id=",
            "<drawing r:id=",
            "<legacyDrawing r:id=",
        ] {
            assert!(s1.contains(e), "{e} missing in {s1}");
        }
        // Schema order: sheetPr first, pageSetup after IronCalc's pageMargins.
        assert!(s1.find("<sheetPr").unwrap() < s1.find("<dimension").unwrap());
        assert!(s1.find("<pageMargins").unwrap_or(0) < s1.find("<pageSetup").unwrap());
        assert!(s1.find("<drawing").unwrap() < s1.find("<legacyDrawing").unwrap());
        assert_eq!(
            sheet_target(&parts, "xl/worksheets/sheet1.xml", "comments").as_deref(),
            Some("xl/comments1.xml")
        );
        assert_eq!(
            sheet_target(&parts, "xl/worksheets/sheet2.xml", "comments").as_deref(),
            Some("xl/comments2.xml")
        );
        // IronCalc reads the comments back.
        let again = model(&done.bytes);
        let texts: Vec<_> = again
            .workbook
            .worksheets
            .iter()
            .map(|w| {
                w.comments
                    .iter()
                    .map(|c| c.text.clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            texts,
            vec![vec!["first".to_owned()], vec!["second".to_owned()]]
        );
        // Saving the saved file keeps everything again.
        let twice = save(&again, Some(&done.bytes));
        assert_eq!(
            parts_of(&twice.bytes).keys().collect::<Vec<_>>(),
            parts.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn parts_follow_their_sheet() {
        let original = rich_fixture("");
        // Reorder and rename: "Other" first, renamed.
        let mut m = model(&original);
        m.move_sheet(1, 0).unwrap();
        m.rename_sheet("Other", "Renamed").unwrap();
        let parts = parts_of(&save(&m, Some(&original)).bytes);
        let wb = text(&parts, "xl/workbook.xml");
        assert!(wb.find("Renamed").unwrap() < wb.find("Data").unwrap());
        assert_eq!(
            sheet_target(&parts, "xl/worksheets/sheet1.xml", "comments").as_deref(),
            Some("xl/comments2.xml")
        );
        assert_eq!(
            sheet_target(&parts, "xl/worksheets/sheet2.xml", "comments").as_deref(),
            Some("xl/comments1.xml")
        );
        assert!(text(&parts, "xl/worksheets/sheet2.xml").contains("<dataValidations"));
        assert!(!text(&parts, "xl/worksheets/sheet1.xml").contains("<dataValidations"));

        // Delete "Data": its parts go, "Other" keeps its own.
        let mut m = model(&original);
        m.delete_sheet(0).unwrap();
        let done = save(&m, Some(&original));
        let parts = parts_of(&done.bytes);
        for gone in [
            "xl/comments1.xml",
            "xl/drawings/drawing1.xml",
            "xl/charts/chart1.xml",
            "xl/printerSettings/printerSettings1.bin",
        ] {
            assert!(!parts.contains_key(gone), "{gone} kept");
        }
        assert_eq!(
            sheet_target(&parts, "xl/worksheets/sheet1.xml", "comments").as_deref(),
            Some("xl/comments2.xml")
        );
        assert!(
            done.notes
                .iter()
                .any(|n| n.contains("\"Data\" was deleted")),
            "{:?}",
            done.notes
        );

        // A new sheet gets nothing.
        let mut m = model(&original);
        m.new_sheet();
        let parts = parts_of(&save(&m, Some(&original)).bytes);
        assert!(!parts.contains_key("xl/worksheets/_rels/sheet3.xml.rels"));
    }

    #[test]
    fn ambiguous_elements_are_skipped_not_mangled() {
        // A foreign element inside <headerFooter>, and an element whose
        // relationship id doesn't exist.
        let original = rich_fixture(
            "<headerFooter><odd>x</odd><foo:bar xmlns:foo=\"urn:x\"/></headerFooter><picture r:id=\"rId99\"/>",
        );
        let done = save(&model(&original), Some(&original));
        let s1 = text(&parts_of(&done.bytes), "xl/worksheets/sheet1.xml");
        assert!(!s1.contains("headerFooter") && !s1.contains("<picture"));
        assert!(s1.contains("<dataValidations"), "the rest is still kept");
        assert!(
            done.notes
                .iter()
                .any(|n| n.contains("<headerFooter> not kept"))
        );
        assert!(done.notes.iter().any(|n| n.contains("<picture> not kept")));
    }

    #[test]
    fn broken_original_links_are_dropped() {
        // The original's drawing points at a chart that isn't in the package.
        let mut pkg = Package::read(&rich_fixture("")).unwrap();
        let i = pkg.index["xl/charts/chart1.xml"];
        pkg.entries.remove(i);
        pkg.index = pkg
            .entries
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.to_lowercase(), i))
            .collect();
        let original = pkg.write().unwrap();
        let done = save(&model(&original), Some(&original));
        let parts = parts_of(&done.bytes);
        assert!(!parts.contains_key("xl/drawings/drawing1.xml"));
        assert!(parts.contains_key("xl/comments1.xml"));
        assert!(done.notes.iter().any(|n| n.contains("<drawing> not kept")));
    }

    #[test]
    fn tab_colors_are_written() {
        let xml = format!("<worksheet xmlns=\"{NS_MAIN}\"><dimension ref=\"A1\"/></worksheet>");
        let set = set_tab_color(&xml, Some("rgb=\"FF112233\""))
            .unwrap()
            .unwrap();
        assert!(set.contains("<sheetPr><tabColor rgb=\"FF112233\"/></sheetPr><dimension"));
        assert_eq!(set_tab_color(&set, Some("rgb=\"FF112233\"")).unwrap(), None);
        let cleared = set_tab_color(&set, None).unwrap().unwrap();
        assert!(cleared.contains("<sheetPr/>") || cleared.contains("<sheetPr></sheetPr>"));
        assert_eq!(set_tab_color(&xml, None).unwrap(), None);
    }

    #[test]
    fn paths() {
        assert_eq!(rels_part(""), "_rels/.rels");
        assert_eq!(
            rels_part("xl/worksheets/sheet1.xml"),
            "xl/worksheets/_rels/sheet1.xml.rels"
        );
        assert_eq!(
            resolve("xl/worksheets/sheet1.xml", "../comments1.xml").as_deref(),
            Some("xl/comments1.xml")
        );
        assert_eq!(
            resolve("xl/workbook.xml", "/docProps/x.xml").as_deref(),
            Some("docProps/x.xml")
        );
        assert_eq!(resolve("", "../../x.xml"), None);
        assert_eq!(resolve("xl/workbook.xml", "a%20b.xml"), None);
        assert_eq!(
            relative("xl/worksheets/sheet2.xml", "xl/comments1.xml"),
            "../comments1.xml"
        );
        assert_eq!(
            relative("xl/worksheets/sheet2.xml", "xl/worksheets/x.bin"),
            "x.bin"
        );
        assert_eq!(relative("", "docProps/custom.xml"), "docProps/custom.xml");
        assert_eq!(
            relative("xl/workbook.xml", "customXml/item1.xml"),
            "../customXml/item1.xml"
        );
    }

    #[test]
    fn invalid_error_values_are_replaced() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="e" s="2"><f>SUM(Jan:Mar!B2)</f><v>#ERROR!</v></c><c r="B1" t="e"><v>#N/IMPL</v></c><c r="C1" t="e"><f>1/0</f><v>#DIV/0!</v></c><c r="D1" t="str"><f>"&lt;#ERROR!&gt;"</f><v>&lt;#ERROR!&gt;</v></c></row></sheetData></worksheet>"#;
        assert!(has_invalid_error(xml));
        let fixed = fix_sheet_errors(xml).unwrap();
        assert_eq!((fixed.formulas, fixed.constants), (1, 1));
        let fixed = fixed.xml;
        assert!(
            fixed.contains(r#"<c r="A1" s="2"><f>SUM(Jan:Mar!B2)</f></c>"#),
            "{fixed}"
        );
        assert!(
            fixed.contains(r#"<c r="B1" t="e"><v>#VALUE!</v></c>"#),
            "{fixed}"
        );
        assert!(fixed.contains(r#"<v>#DIV/0!</v>"#));
        assert!(fixed.contains(r#"<f>"&lt;#ERROR!&gt;"</f>"#));
        assert!(!has_invalid_error(&fixed));
    }

    #[test]
    fn serializer_maps_ids_and_drops_foreign_attributes() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:xr="http://schemas.microsoft.com/office/spreadsheetml/2014/revision"><pageSetup orientation="landscape" r:id="rId7"/><dataValidations count="1"><dataValidation type="list" sqref="A1" xr:uid="{X}"><formula1>"a,b"</formula1></dataValidation></dataValidations></worksheet>"#;
        let doc = Document::parse(xml).unwrap();
        let mut kids = doc.root_element().children();
        let ids = HashMap::from([("rId7".to_owned(), "rId2".to_owned())]);
        let mut out = String::new();
        write_node(kids.next().unwrap(), &mut out, &ids).unwrap();
        assert_eq!(out, r#"<pageSetup orientation="landscape" r:id="rId2"/>"#);
        out.clear();
        write_node(kids.next().unwrap(), &mut out, &ids).unwrap();
        assert_eq!(
            out,
            r#"<dataValidations count="1"><dataValidation type="list" sqref="A1"><formula1>"a,b"</formula1></dataValidation></dataValidations>"#
        );
    }

    #[test]
    fn problems_are_found() {
        let mut pkg = Package::default();
        pkg.set(
            CONTENT_TYPES,
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/gone.xml" ContentType="application/xml"/></Types>"#.to_vec(),
        );
        pkg.set(
            "_rels/.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="t" Target="a.xml"/><Relationship Id="rId1" Type="t" Target="missing.xml"/></Relationships>"#.to_vec(),
        );
        pkg.set("a.xml", b"<a/>".to_vec());
        pkg.set("b.bin", b"x".to_vec());
        let problems = package_problems(&pkg.write().unwrap());
        assert_eq!(
            problems,
            vec![
                "b.bin: no content type".to_owned(),
                "content type override for missing part /gone.xml".to_owned(),
                "_rels/.rels: duplicate id rId1".to_owned(),
                "_rels/.rels: target missing.xml of rId1 is missing".to_owned(),
            ]
        );
    }
}
