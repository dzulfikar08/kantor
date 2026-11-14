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
//!      threaded comments, pivot tables with their caches and records; the
//!      caches a kept pivot table uses are listed again in the workbook's
//!      `<pivotCaches>`, see [`carry_pivot_caches`]; slicers and timelines:
//!      their sheet and workbook extensions with the parts they link to, see
//!      [`LINKED_EXTENSIONS`] and [`carry_workbook_extensions`]), and a fixed set of sheet elements IronCalc doesn't
//!      write ([`CARRIED_ELEMENTS`]: data validation, print setup, drawings,
//!      `legacyDrawing` for comment boxes, `tableParts`, `extLst`...) with the
//!      parts they point to. Tables are checked against the saved cells: an
//!      edited header renames its column, an empty or duplicate one drops the
//!      table. Of `extLst`, the extensions IronCalc doesn't write itself are
//!      added (sparklines, Excel 2010 data validations...), with the namespace
//!      declarations they inherited;
//!    * conditional formats: the original's `<conditionalFormatting>`
//!      elements, its Excel 2010 conditional formatting extension and the
//!      `<dxfs>` of its styles replace what IronCalc writes from its model
//!      (which drops attributes such as icon order and "icon only", and
//!      rewrites text rules); see [`carry_conditional_formats`];
//!    * chart sheets (IronCalc has none): the part with its drawing and chart,
//!      put back into the sheet list after the sheet that preceded it, with
//!      `localSheetId` and `activeTab` renumbered; a chart sheet gets a new
//!      `sheetId` if a sheet added in Kantor took its id;
//!    * defined names: attributes IronCalc drops (`hidden` on
//!      `_xlnm._FilterDatabase`, comments...), and built-in or hidden names it
//!      didn't write.
//!
//!    Original sheets are matched to saved ones by `sheetId` (IronCalc keeps it;
//!    renames and reorders keep the match). Parts of a sheet that is gone are
//!    dropped. Caveat: IronCalc gives a new sheet `max(sheetId) + 1`, so
//!    deleting the *last-numbered* sheet and then adding one reuses its id and
//!    the new sheet would inherit the old one's extras; an add-sheet API in
//!    `sheets_engine` must pick ids above every id the original had.
//! 3. **Inserted and deleted rows and columns** (`crate::shift`): what is
//!    carried follows them, as Excel would move it: `sqref`/`ref` ranges of
//!    validations, filters, protected ranges and ignored errors, page breaks,
//!    validation formulas, conditional format ranges and formulas, table
//!    ranges, sparkline locations and data,
//!    comment cells and their boxes, drawing anchors, chart series on any
//!    sheet, pivot table locations and pivot cache source ranges. Something whose cells were all deleted goes (a note says so); a
//!    table with columns inserted or deleted inside it, or without its header
//!    row, is dropped rather than left for Excel to repair. A part that can't
//!    be read to move it (e.g. VML that isn't XML) is kept as it was, with a
//!    note. Not followed: sheet renames inside carried formulas (Kantor can't
//!    rename sheets yet), external links (never carried).
//!
//! Everything is conservative: anything ambiguous (unknown markup around a
//! reference, name collisions, broken links in the original) is skipped and
//! reported in the notes. The merged package is only used if it passes
//! [`package_problems`] (no worse than IronCalc's own output) and IronCalc can
//! read it back; otherwise IronCalc's output (with fixed error values) is saved.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::rc::Rc;

use roxmltree::{Document, Node};

use crate::shift::{Moves, parse_range};

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
/// defaults (row height, column width) Kantor can't change. `tableParts` is
/// carried table by table ([`carry_tables`]: header cells must still match
/// the column names, or Excel repairs the file), `extLst` extension by
/// extension ([`carry_extensions`]). `autoFilter` is not carried: IronCalc
/// writes the filter state Kantor keeps in the model (`crate::filter`).
/// Conditional formats replace IronCalc's
/// own ([`carry_conditional_formats`]). Left out on purpose: what IronCalc
/// models (views, columns, merges, hyperlinks) and
/// `oleObjects`/`controls` (built on `mc:AlternateContent`).
pub(crate) const CARRIED_ELEMENTS: [&str; 18] = [
    "sheetPr",
    "sheetFormatPr",
    "sheetProtection",
    "protectedRanges",
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
    "tableParts",
    "extLst",
];

/// The `extLst` extension of Excel 2010 conditional formats (x14 data bars,
/// icon sets, rules with formulas on other sheets).
const CF_EXTENSION: &str = "{78C0D931-6437-407d-A8EE-F0AAD7539E65}";

/// Relationship types never carried, whatever level they're on: stale caches,
/// or parts that only work with markup IronCalc rewrites.
const NEVER_CARRY: [&str; 7] = [
    "calcChain",
    "worksheet",
    "chartsheet",
    "dialogsheet",
    "externalLink",
    "revisionHeaders",
    "vbaProject",
];

/// Sheet extensions that link to parts and are carried with them: slicer
/// lists (pivot table slicers, Excel 2010; table slicers, Excel 2013) and
/// timelines (Excel 2013). The caches they use are workbook extensions
/// ([`WORKBOOK_EXTENSIONS`]); the slicers' drawing anchors are carried with
/// the sheet's drawing.
const LINKED_EXTENSIONS: [&str; 3] = [
    "{A8765BA9-456A-4dab-B4F3-ACF838C121DE}",
    "{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}",
    "{7E03D99C-DC04-49d9-9315-930204A7B6E9}",
];

/// Workbook extensions carried with the parts they link to: slicer caches
/// (Excel 2010 pivot slicers, Excel 2013 table slicers) and timeline caches.
/// IronCalc writes no workbook `<extLst>`.
const WORKBOOK_EXTENSIONS: [&str; 3] = [
    "{BBE1A952-AA13-448e-AADC-164F8A28A991}",
    "{46BE6895-7355-4a93-B00E-2C351335B9C9}",
    "{D0CA8CA8-9F24-4464-BF8E-62219DCF47F9}",
];

/// `xml` (one element) with its relationship ids (`r:id`...) renamed by `map`.
fn rename_rel_ids(xml: &str, map: &HashMap<String, String>) -> Result<String, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let mut edits = Vec::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        for a in n.attributes() {
            if matches!(a.namespace(), Some(NS_R) | Some(NS_R_STRICT)) {
                let new = map
                    .get(a.value())
                    .ok_or_else(|| format!("relationship {} is not planned", a.value()))?;
                edits.push((a.range_value(), esc_attr(new)));
            }
        }
    }
    Ok(apply_edits(xml, edits))
}

/// Largest part read from the original (guards against zip bombs).
const MAX_PART: u64 = 512 * 1024 * 1024;

/// The bytes to save and what preservation had to report.
pub(crate) struct Finished {
    pub bytes: Vec<u8>,
    pub notes: Vec<String>,
    pub pivots: PivotsSaved,
}

/// A sheet's tab colour as the model has it, by `sheetId`: the attributes of
/// `<tabColor>` (e.g. `rgb="FF4472C4"`), `None` for no colour.
pub(crate) type TabColors = HashMap<u32, Option<String>>;

/// What the workbook being saved tells the post-processing.
pub(crate) struct Context<'a> {
    pub tabs: &'a TabColors,
    /// Ask Excel to recalculate when it opens the file (`fullCalcOnLoad`),
    /// for workbooks saved with values that may be stale.
    pub full_calc: bool,
    /// Rows and columns inserted or deleted since `original` was read.
    pub moves: &'a Moves,
    /// The text a cell shows, by `(sheetId, row, column)` (0-based).
    pub cell_text: &'a dyn Fn(u32, u32, u32) -> String,
    /// The tables of the saved workbook, by name (any case): where a table
    /// carried from the original is now, and its filter.
    pub tables: &'a dyn Fn(&str) -> Option<TableNow>,
    /// Drawing objects moved, resized or deleted, by `sheetId`.
    pub drawings: &'a HashMap<u32, Vec<crate::drawing::SaveEdit>>,
    /// Pivot parts rewritten by an edit, pivot tables deleted and pivot
    /// tables made in Kantor.
    pub pivots: &'a PivotSave,
    /// Sheet features edited in Kantor, written over what the package has.
    pub patches: &'a Patches,
}

/// What a save writes over the package for features edited in Kantor (see
/// [`apply_patches`]). Sheets and names not listed are left as they are.
#[derive(Default)]
pub(crate) struct Patches {
    /// By `sheetId`.
    pub sheets: HashMap<u32, SheetPatch>,
    /// Built-in defined names `(sheetId, name, formula)`: written for that
    /// sheet in place of any the workbook has, or removed (`None`).
    pub names: Vec<(u32, &'static str, Option<String>)>,
    /// Tables made or changed in Kantor: their parts are written from the
    /// model (an existing part with one of their names is rewritten, else a
    /// new part is added).
    pub tables: Vec<TableSave>,
    /// Lower-case names of tables whose original parts aren't carried
    /// (converted to a range, or renamed: written again under the new name).
    pub removed_tables: HashSet<String>,
    /// Lower-case names of tables changed in Kantor: carried as they are and
    /// rewritten afterwards, without the checks of [`fit_table`].
    pub touched_tables: HashSet<String>,
    /// Sheets (`sheetId`) whose conditional formats were changed in Kantor:
    /// IronCalc's markup from the model is kept for them (and the dxfs it
    /// added are appended to the original's).
    pub cf_sheets: HashSet<u32>,
    /// Charts made in Kantor, added with their drawing.
    pub charts: Vec<ChartSave>,
    /// Charts an earlier save of this session added that are gone since.
    pub removed_charts: Vec<SavedChart>,
    /// Filled by the save: where each chart made in Kantor went, by id.
    pub written: RefCell<Vec<(u32, SavedChart)>>,
    /// Charts of the file restyled in Kantor: their parts are rewritten.
    pub file_charts: Vec<crate::file_chart::FileChartSave>,
    /// Notes of an import, by `sheetId`: written with their VML boxes to
    /// sheets that have none.
    pub notes: Vec<(u32, Vec<crate::notes::NoteSave>)>,
}

/// Where a save put a chart made in Kantor (the next save of the same
/// session updates or removes it there: the saved file is its original).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SavedChart {
    pub drawing: String,
    pub cnvpr: u32,
    pub part: String,
    pub rid: String,
}

/// A chart made in Kantor, to add to its sheet's drawing.
pub(crate) struct ChartSave {
    pub id: u32,
    /// Where the last save put it, if it did.
    pub saved: Option<SavedChart>,
    pub sheet_id: u32,
    pub name: String,
    pub anchor: crate::drawing::Anchor,
    /// The `c:chartSpace` part.
    pub xml: String,
}

/// A table to write from the model.
pub(crate) struct TableSave {
    pub sheet_id: u32,
    /// Its name, then names it had before.
    pub names: Vec<String>,
    pub table: ironcalc_base::types::Table,
}

/// Changes to one worksheet part.
#[derive(Default)]
pub(crate) struct SheetPatch {
    /// Elements of `<worksheet>` to write in place of the package's (put in
    /// schema order when it has none), `None` to remove them.
    pub elements: Vec<(&'static str, Option<String>)>,
    /// `<sheetPr><pageSetUpPr fitToPage>`.
    pub fit_to_page: Option<bool>,
    /// `<pageSetup>`: the attributes Kantor models
    /// (`crate::print::PAGE_SETUP_ATTRS`) are set to these; others kept.
    pub page_setup: Option<Vec<(&'static str, String)>>,
    /// `extLst` extensions (by `uri`) to remove.
    pub remove_exts: Vec<&'static str>,
    /// Frozen panes frozen on a scrolled sheet: `(origin, split)`, the
    /// panes' top-left cell and the first unfrozen row and column, counted
    /// from A1 (`grid::set_frozen_at`). Written as the sheet view's
    /// `topLeftCell` and a pane whose splits count from it.
    pub frozen_view: Option<((u32, u32), (u32, u32))>,
}

impl Patches {
    fn is_empty(&self) -> bool {
        self.sheets.is_empty()
            && self.names.is_empty()
            && self.tables.is_empty()
            && self.charts.is_empty()
            && self.removed_charts.is_empty()
            && self.file_charts.is_empty()
            && self.notes.is_empty()
    }
}

/// The pivot parts of the workbook being saved, once pivot tables were
/// refreshed, made, rearranged or deleted.
#[derive(Default)]
pub(crate) struct PivotSave {
    /// Table definitions, cache definitions and records rewritten, by part
    /// name: saved instead of the original's, as they are (already placed
    /// where their cells are now).
    pub replaced: HashMap<String, Vec<u8>>,
    /// Pivot table parts of the original not to carry (deleted), lower case.
    pub dropped: HashSet<String>,
    /// Pivot tables whose parts the original doesn't have.
    pub added: Vec<NewPivot>,
}

/// A pivot table to add to the package with its cache (see
/// [`add_pivots`]). Part names are proposals: a name the package already
/// has gets the next free number.
pub(crate) struct NewPivot {
    pub sheet_id: u32,
    pub table_part: String,
    /// Its `cacheId` is set to the cache's.
    pub table_xml: String,
    /// Several new pivot tables may share a cache (same part).
    pub cache_part: String,
    /// Its root's `r:id` names the records.
    pub cache_xml: String,
    pub records_part: Option<String>,
    pub records_xml: Option<String>,
}

/// What a save did with pivot tables.
#[derive(Default, Debug)]
pub(crate) struct PivotsSaved {
    /// New pivot tables, with the names their parts got.
    pub added: Vec<AddedPivot>,
    /// Pivot table parts of the saved package, lower case.
    pub tables: HashSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AddedPivot {
    pub proposed_table: String,
    pub table: String,
    pub proposed_cache: String,
    pub cache: String,
    pub records: Option<String>,
}

/// A table as the model has it at save time.
pub(crate) struct TableNow {
    pub sheet_id: u32,
    /// `A1:D10`.
    pub reference: String,
    /// The `<autoFilter>` element, its `ref` still to be set.
    pub auto_filter: Option<String>,
    pub totals_rows: u32,
    /// Column ids and names, in order.
    pub columns: Vec<(u32, String)>,
}

/// Post-process IronCalc's xlsx: carry over what the `original` package had
/// and IronCalc dropped, write tab colours, fix error values. Never fails: on
/// any problem the result falls back to IronCalc's own output (with tab
/// colours and error values fixed when possible).
pub(crate) fn finish(ironcalc: Vec<u8>, original: Option<&[u8]>, ctx: &Context) -> Finished {
    let (tabs, full_calc) = (ctx.tabs, ctx.full_calc);
    let mut notes = Vec::new();
    let new = match Package::read(&ironcalc) {
        Ok(p) => p,
        Err(e) => {
            notes.push(format!("could not post-process the saved package: {e}"));
            return Finished {
                bytes: ironcalc,
                notes,
                pivots: PivotsSaved::default(),
            };
        }
    };
    let new_pivots = &ctx.pivots.added;
    // The baseline: IronCalc's package with the fixes every save gets (and
    // the pivot tables made in Kantor). Made only if it is what gets saved
    // (from IronCalc's bytes again, so a big workbook's sheets are not held
    // twice).
    let baseline = |notes: &mut Vec<String>| -> (Vec<u8>, Vec<AddedPivot>) {
        let mut added = Vec::new();
        let polished = Package::read(&ironcalc).and_then(|mut b| {
            polish(&mut b, tabs, full_calc, notes)?;
            if let Err(e) = apply_patches(&mut b, ctx.patches) {
                notes.push(format!(
                    "Kantor's page setup and other sheet edits are not saved: {e}"
                ));
                b = Package::read(&ironcalc)?;
                polish(&mut b, tabs, full_calc, notes)?;
            }
            if !new_pivots.is_empty() {
                let before = b.clone();
                match add_pivots(&mut b, new_pivots).and_then(|a| {
                    let problems = problems_of(&b);
                    if problems.is_empty() {
                        Ok(a)
                    } else {
                        Err(problems.join("; "))
                    }
                }) {
                    Ok(a) => added = a,
                    Err(e) => {
                        notes.push(format!("the new PivotTables are not saved: {e}"));
                        b = before;
                    }
                }
            }
            b.write()
        });
        match polished {
            Ok(bytes) => (bytes, added),
            Err(e) => {
                notes.push(format!("could not post-process the saved package: {e}"));
                (ironcalc.clone(), Vec::new())
            }
        }
    };
    let done = |bytes: Vec<u8>, notes: Vec<String>, added: Vec<AddedPivot>| {
        let tables = Package::read(&bytes)
            .map(|p| {
                p.parts()
                    .map(str::to_lowercase)
                    .filter(|n| n.starts_with("xl/pivottables/") && !n.contains("/_rels/"))
                    .collect()
            })
            .unwrap_or_default();
        Finished {
            bytes,
            notes,
            pivots: PivotsSaved { added, tables },
        }
    };
    let Some(original) = original else {
        drop(new);
        let (bytes, added) = baseline(&mut notes);
        return done(bytes, notes, added);
    };

    // Polishing doesn't change the package structure.
    let before: HashSet<String> = problems_of(&new).into_iter().collect();
    let mut merged = new;
    let mut merge_notes = Vec::new();
    let mut polish_notes = Vec::new();
    let mut added = Vec::new();
    let polished = match merge(&mut merged, original, ctx, &mut merge_notes) {
        Ok(false) => {
            drop(merged);
            notes.extend(merge_notes);
            let (bytes, added) = baseline(&mut notes);
            return done(bytes, notes, added);
        }
        Ok(true) => polish(&mut merged, tabs, full_calc, &mut polish_notes).and_then(|()| {
            apply_patches(&mut merged, ctx.patches)?;
            added = add_pivots(&mut merged, new_pivots)?;
            Ok(())
        }),
        Err(e) => Err(e),
    };
    notes.extend(merge_notes);
    let checked = polished.and_then(|()| {
        let added: Vec<String> = problems_of(&merged)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        if !added.is_empty() {
            return Err(format!("the result would be invalid: {}", added.join("; ")));
        }
        read_back(&merged, READ_BACK_FULL)
    });
    drop(merged);
    match checked {
        Ok(bytes) => {
            notes.extend(polish_notes);
            done(bytes, notes, added)
        }
        Err(e) => {
            notes.push(format!("not keeping the original's extra parts: {e}"));
            let (bytes, added) = baseline(&mut notes);
            done(bytes, notes, added)
        }
    }
}

const PIVOT_TABLE_CT: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml";
const PIVOT_CACHE_CT: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml";
const PIVOT_RECORDS_CT: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheRecords+xml";
const REL_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";

/// `wanted` if the package has no such part, else the next free name with
/// the same stem (`xl/pivotTables/pivotTable` + number + `.xml`).
fn free_part(pkg: &Package, wanted: &str, stem: &str) -> String {
    if !wanted.starts_with(crate::pivot::NEW_PREFIX) && !pkg.has(wanted) {
        return wanted.to_owned();
    }
    (1..)
        .map(|n| format!("{stem}{n}.xml"))
        .find(|n| !pkg.has(n))
        .expect("a free name")
}

/// `xml` with attribute `name` (no namespace) of its root element set to
/// `value`.
fn set_root_attr(xml: &str, name: &str, value: &str) -> Result<String, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if let Some(a) = root
        .attributes()
        .find(|a| a.namespace().is_none() && a.name() == name)
    {
        return Ok(apply_edits(xml, vec![(a.range_value(), esc_attr(value))]));
    }
    let start = root.range().start;
    let at = start
        + 1
        + xml[start + 1..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .ok_or("root element not closed")?;
    Ok(apply_edits(
        xml,
        vec![(at..at, format!(" {name}=\"{}\"", esc_attr(value)))],
    ))
}

/// Add pivot tables made in Kantor with their caches: the parts, their
/// relationships (sheet → table → cache → records, workbook → cache), the
/// workbook's `<pivotCaches>` and the content types. Returns the names the
/// parts got.
fn add_pivots(pkg: &mut Package, adds: &[NewPivot]) -> Result<Vec<AddedPivot>, String> {
    if adds.is_empty() {
        return Ok(Vec::new());
    }
    let root_rels = parse_rels(pkg.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let read = |p: &str| Ok(pkg.text(p)?.map(str::to_owned));
    let (wb, mut wb_rels, wb_xml, sheets) = workbook_sheets(&root_rels, &read)?;
    let mut wb_ids = IdAlloc::new(&wb_rels);
    let wb_doc = Document::parse(&wb_xml).map_err(|e| format!("{wb}: {e}"))?;
    let wb_root = wb_doc.root_element();
    if wb_root.lookup_namespace_uri(Some("r")) != Some(NS_R) {
        return Err("the workbook part has no r: prefix".into());
    }
    let list = wb_root.children().find(|c| is_main(*c, "pivotCaches"));
    let mut next_id = list
        .map(|l| {
            l.children()
                .filter(|c| is_main(*c, "pivotCache"))
                .filter_map(|c| c.attribute("cacheId")?.parse::<u32>().ok())
                .max()
                .map_or(0, |m| m + 1)
        })
        .unwrap_or(0);
    let mut ct = String::new();
    let mut caches_text = String::new();
    // Proposed cache part -> (cacheId, part, records part).
    let mut caches: HashMap<String, (u32, String, Option<String>)> = HashMap::new();
    let mut sheet_rels: HashMap<String, Vec<Rel>> = HashMap::new();
    let mut out = Vec::new();
    for a in adds {
        let sheet = sheets
            .iter()
            .find(|s| s.sheet_id == a.sheet_id && s.kind == SheetKind::Work)
            .ok_or_else(|| format!("PivotTable sheet {} is missing", a.sheet_id))?;
        let (cache_id, cache_part, records_part) = match caches.get(&a.cache_part) {
            Some(c) => c.clone(),
            None => {
                let cache_part =
                    free_part(pkg, &a.cache_part, "xl/pivotCache/pivotCacheDefinition");
                let mut cache_xml = a.cache_xml.clone();
                let mut records_part = None;
                if let Some(records) = &a.records_xml {
                    let doc = Document::parse(&cache_xml).map_err(|e| e.to_string())?;
                    let rid = doc
                        .root_element()
                        .attribute((NS_R, "id"))
                        .ok_or("the cache names no records")?
                        .to_owned();
                    drop(doc);
                    pkg.set(&cache_part, Vec::new());
                    let rp = free_part(
                        pkg,
                        a.records_part
                            .as_deref()
                            .unwrap_or(crate::pivot::NEW_PREFIX),
                        "xl/pivotCache/pivotCacheRecords",
                    );
                    pkg.set(&rp, records.clone().into_bytes());
                    pkg.set(
                        &rels_part(&cache_part),
                        write_rels(&[Rel {
                            id: rid,
                            kind: format!("{REL_BASE}pivotCacheRecords"),
                            target: relative(&cache_part, &rp),
                            external: false,
                        }])
                        .into_bytes(),
                    );
                    ct.push_str(&format!(
                        "<Override PartName=\"/{}\" ContentType=\"{PIVOT_RECORDS_CT}\"/>",
                        esc_attr(&rp)
                    ));
                    records_part = Some(rp);
                } else {
                    // No records: the cache must not point at any.
                    let doc = Document::parse(&cache_xml).map_err(|e| e.to_string())?;
                    if doc.root_element().attribute((NS_R, "id")).is_some() {
                        return Err("the cache's records are missing".into());
                    }
                }
                cache_xml = cache_xml.trim_start_matches('\u{feff}').to_owned();
                pkg.set(&cache_part, cache_xml.into_bytes());
                ct.push_str(&format!(
                    "<Override PartName=\"/{}\" ContentType=\"{PIVOT_CACHE_CT}\"/>",
                    esc_attr(&cache_part)
                ));
                let rid = wb_ids.next();
                wb_rels.push(Rel {
                    id: rid.clone(),
                    kind: format!("{REL_BASE}pivotCacheDefinition"),
                    target: relative(&wb, &cache_part),
                    external: false,
                });
                caches_text.push_str(&format!(
                    "<pivotCache cacheId=\"{next_id}\" r:id=\"{}\"/>",
                    esc_attr(&rid)
                ));
                let entry = (next_id, cache_part, records_part);
                next_id += 1;
                caches.insert(a.cache_part.clone(), entry.clone());
                entry
            }
        };
        let table = free_part(pkg, &a.table_part, "xl/pivotTables/pivotTable");
        let xml = set_root_attr(&a.table_xml, "cacheId", &cache_id.to_string())?;
        pkg.set(&table, xml.into_bytes());
        pkg.set(
            &rels_part(&table),
            write_rels(&[Rel {
                id: "rId1".into(),
                kind: format!("{REL_BASE}pivotCacheDefinition"),
                target: relative(&table, &cache_part),
                external: false,
            }])
            .into_bytes(),
        );
        ct.push_str(&format!(
            "<Override PartName=\"/{}\" ContentType=\"{PIVOT_TABLE_CT}\"/>",
            esc_attr(&table)
        ));
        let rels = match sheet_rels.get_mut(&sheet.part) {
            Some(r) => r,
            None => {
                let existing = match pkg.text(&rels_part(&sheet.part))? {
                    Some(x) => parse_rels(x)?,
                    None => Vec::new(),
                };
                sheet_rels.entry(sheet.part.clone()).or_insert(existing)
            }
        };
        let id = IdAlloc::new(rels).next();
        rels.push(Rel {
            id,
            kind: format!("{REL_BASE}pivotTable"),
            target: relative(&sheet.part, &table),
            external: false,
        });
        out.push(AddedPivot {
            proposed_table: a.table_part.clone(),
            table,
            proposed_cache: a.cache_part.clone(),
            cache: cache_part,
            records: records_part,
        });
    }
    for (part, rels) in sheet_rels {
        pkg.set(&rels_part(&part), write_rels(&rels).into_bytes());
    }
    pkg.set(&rels_part(&wb), write_rels(&wb_rels).into_bytes());
    // The workbook's cache list (CT_Workbook: after sheets, functionGroups,
    // externalReferences, definedNames, calcPr, oleSize,
    // customWorkbookViews).
    let edit = match list {
        Some(l) => {
            let at = l.range().end - "</pivotCaches>".len();
            if !wb_xml[..l.range().end].ends_with("</pivotCaches>") {
                return Err("the workbook's <pivotCaches> is not closed as expected".into());
            }
            (at..at, caches_text)
        }
        None => {
            let before = [
                "sheets",
                "functionGroups",
                "externalReferences",
                "definedNames",
                "calcPr",
                "oleSize",
                "customWorkbookViews",
            ];
            let last = wb_root
                .children()
                .rfind(|c| before.iter().any(|n| is_main(*c, n)))
                .ok_or("the workbook has no <sheets>")?
                .range()
                .end;
            (
                last..last,
                format!("<pivotCaches>{caches_text}</pivotCaches>"),
            )
        }
    };
    let new_wb = apply_edits(&wb_xml, vec![edit]);
    drop(wb_doc);
    pkg.set(&wb, new_wb.into_bytes());
    let types = pkg
        .text(CONTENT_TYPES)?
        .ok_or("no content types")?
        .to_owned();
    let at = types.rfind("</Types>").ok_or("content types not closed")?;
    let mut types = types;
    types.insert_str(at, &ct);
    pkg.set(CONTENT_TYPES, types.into_bytes());
    Ok(out)
}

/// Worksheet XML (uncompressed) above which [`read_back`] checks the
/// package without its cells.
const READ_BACK_FULL: usize = 16 * 1024 * 1024;

/// The bytes of `pkg`, once IronCalc has read them back. A big package is
/// read without the inside of `<sheetData>` (its cells are IronCalc's own
/// output, untouched by the merge apart from error values): reading 200 MB of
/// cells again costs seconds and hundreds of MB for nothing.
fn read_back(pkg: &Package, full_limit: usize) -> Result<Vec<u8>, String> {
    let is_sheet = |n: &str| {
        let n = n.to_lowercase();
        n.starts_with("xl/worksheets/") && n.ends_with(".xml")
    };
    let sheet_bytes: usize = pkg
        .entries
        .iter()
        .filter(|(n, _)| is_sheet(n))
        .map(|(_, d)| d.len())
        .sum();
    let bytes = pkg.write()?;
    let check = if sheet_bytes <= full_limit {
        ironcalc::import::load_from_xlsx_bytes(&bytes, "check", "en", "UTC").map(|_| ())
    } else {
        let mut light = pkg.clone();
        let names: Vec<String> = pkg
            .entries
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| is_sheet(n))
            .collect();
        for name in names {
            let xml = light.text(&name)?.unwrap_or_default();
            let skeleton = Skeleton::new(xml).xml;
            light.set(&name, skeleton.into_bytes());
        }
        ironcalc::import::load_from_xlsx_bytes(&light.write()?, "check", "en", "UTC").map(|_| ())
    };
    check.map_err(|e| format!("IronCalc can't read the result: {e}"))?;
    Ok(bytes)
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
    /// Shared, so a copy of the package costs little until parts change.
    entries: Vec<(String, Rc<Vec<u8>>)>,
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

    /// A part's text, taken out of the package (empty until set again):
    /// no copy when nothing else shares it.
    fn take_text(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(&i) = self.index.get(&name.to_lowercase()) else {
            return Ok(None);
        };
        let data = std::mem::take(&mut self.entries[i].1);
        let mut data = Rc::try_unwrap(data).unwrap_or_else(|shared| shared.as_ref().clone());
        if data.starts_with(b"\xEF\xBB\xBF") {
            data.drain(..3);
        }
        String::from_utf8(data)
            .map(Some)
            .map_err(|_| format!("{name} is not UTF-8"))
    }

    fn remove(&mut self, name: &str) {
        let key = name.to_lowercase();
        if self.index.remove(&key).is_none() {
            return;
        }
        self.entries.retain(|(n, _)| n.to_lowercase() != key);
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.to_lowercase(), i))
            .collect();
    }

    fn set(&mut self, name: &str, data: Vec<u8>) {
        match self.index.get(&name.to_lowercase()) {
            Some(&i) => self.entries[i].1 = Rc::new(data),
            None => {
                self.index.insert(name.to_lowercase(), self.entries.len());
                self.entries.push((name.to_owned(), Rc::new(data)));
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

    /// A worksheet's [`Skeleton`], read without holding its cells in memory.
    fn skeleton(&self, name: &str) -> Result<Option<Skeleton>, String> {
        let Some(stored) = self.names.get(&name.to_lowercase()) else {
            return Ok(None);
        };
        let mut zip = self.zip.borrow_mut();
        let entry = zip.by_name(stored).map_err(|e| format!("{name}: {e}"))?;
        let mut reader = entry.take(MAX_PART);
        let mut out: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 1 << 20];
        const OPEN: &[u8] = b"<sheetData";
        const CLOSE: &[u8] = b"</sheetData>";
        // 0: before the cells, 1: skipping them, 2: after them.
        let mut state = 0;
        let mut cut_at = 0;
        let mut skipped = 0;
        let mut tail: Vec<u8> = Vec::new(); // the last bytes skipped
        let mut scan_from = 0;
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("{name}: {e}"))?;
            if n == 0 {
                break;
            }
            let mut chunk = &buf[..n];
            if state == 0 {
                out.extend_from_slice(chunk);
                chunk = &[];
                let found = find_bytes(&out[scan_from..], OPEN).map(|i| i + scan_from);
                // Search again from here next time (a tag can span chunks).
                scan_from = found.unwrap_or(out.len().saturating_sub(OPEN.len()));
                // The end of `<sheetData ...>`, once all of it is in.
                if let Some(open) = found
                    && out.len() > open + OPEN.len()
                    && let Some(gt) = out[open..].iter().position(|&b| b == b'>')
                {
                    let gt = open + gt;
                    let boundary = out.get(open + OPEN.len()).copied().unwrap_or(b'>');
                    if !(boundary == b'>' || boundary == b'/' || boundary.is_ascii_whitespace()) {
                        scan_from = open + 1; // `<sheetDataX`: look further
                    } else if out[gt - 1] == b'/' {
                        state = 2; // empty: keep everything
                    } else {
                        cut_at = gt + 1;
                        let rest = out.split_off(cut_at);
                        state = 1;
                        tail = rest;
                    }
                }
            } else if state == 2 {
                out.extend_from_slice(chunk);
                chunk = &[];
            }
            if state == 1 {
                tail.extend_from_slice(chunk);
                match find_bytes(&tail, CLOSE) {
                    Some(i) => {
                        skipped += i;
                        out.extend_from_slice(&tail[i..]);
                        tail = Vec::new();
                        state = 2;
                    }
                    None => {
                        let keep = CLOSE.len().min(tail.len());
                        skipped += tail.len() - keep;
                        tail.drain(..tail.len() - keep);
                    }
                }
            }
        }
        if state == 1 {
            return Err(format!("{name}: <sheetData> is not closed"));
        }
        let bom = out.starts_with(b"\xEF\xBB\xBF");
        let xml = String::from_utf8(out).map_err(|_| format!("{name} is not UTF-8"))?;
        let (xml, shift) = if bom {
            (xml[3..].to_owned(), 3)
        } else {
            (xml, 0)
        };
        let cut = (skipped > 0).then(|| (cut_at - shift, skipped));
        Ok(Some(Skeleton { xml, cut }))
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
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
    match Package::read(bytes) {
        Ok(pkg) => problems_of(&pkg),
        Err(e) => vec![format!("not a readable zip: {e}")],
    }
}

/// [`package_problems`] of a package in memory.
fn problems_of(pkg: &Package) -> Vec<String> {
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

/// Apply edits from the end of `xml` backwards. At one offset, a replaced
/// element is replaced first and inserts go before it, in the order they
/// were pushed (the schema order of what goes there, e.g. `<pivotCaches>`
/// before `<extLst>`).
fn apply_edits(xml: &str, edits: Vec<(std::ops::Range<usize>, String)>) -> String {
    let mut edits: Vec<_> = edits.into_iter().enumerate().collect();
    edits.sort_by_key(|(i, (r, _))| std::cmp::Reverse((r.start, !r.is_empty(), *i)));
    let mut out = xml.to_owned();
    for (_, (range, text)) in edits {
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
        && !pkg
            .entries
            .iter()
            .any(|(n, d)| n.starts_with("xl/worksheets/") && memmem(d, b"tabColor"))
    {
        return Ok(());
    }
    let root_rels = parse_rels(pkg.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let read = |p: &str| Ok(pkg.text(p)?.map(str::to_owned));
    let (_, _, _, sheets) = workbook_sheets(&root_rels, &read)?;
    for sheet in sheets.into_iter().filter(|s| s.kind == SheetKind::Work) {
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
    write_node_without(node, out, ids, None, None).map(|_| ())
}

/// How carried sheet markup follows rows and columns that moved: ranges in
/// `sqref` and `ref` attributes, page breaks, and validation formulas.
struct Rewrite<'a> {
    moves: &'a Moves,
    /// The sheet the markup is on (`sheetId`).
    home: u32,
    /// Sheet names (lower case) -> `sheetId`, for formulas.
    sheets: &'a HashMap<String, u32>,
}

/// [`write_node`], leaving out the children of `node` named `skip` and
/// moving references with `rw`. Returns `false` if the element was left out
/// because everything it applies to was deleted.
fn write_node_without(
    node: Node,
    out: &mut String,
    ids: &HashMap<String, String>,
    skip: Option<&str>,
    rw: Option<&Rewrite>,
) -> Result<bool, String> {
    if node.is_text() {
        let text = node.text().unwrap_or_default();
        let parent = node
            .parent()
            .map(|p| p.tag_name().name())
            .unwrap_or_default();
        match rw {
            Some(rw) if matches!(parent, "formula" | "formula1" | "formula2") => {
                out.push_str(&esc_text(&rw.moves.formula(text, Some(rw.home), rw.sheets)));
            }
            _ => out.push_str(&esc_text(text)),
        }
        return Ok(true);
    }
    if !node.is_element() {
        return Ok(true);
    }
    let name = node.tag_name();
    if name.namespace() != Some(NS_MAIN) {
        return Err(format!("unsupported element <{}>", name.name()));
    }
    // Attributes that follow moved rows and columns.
    let mut moved: HashMap<&str, String> = HashMap::new();
    if let Some(rw) = rw.filter(|rw| rw.moves.touches(rw.home)) {
        let parent = node
            .parent()
            .map(|p| p.tag_name().name())
            .unwrap_or_default();
        for a in node.attributes().filter(|a| a.namespace().is_none()) {
            let range_attr = a.name() == "sqref"
                || (a.name() == "ref"
                    && matches!(name.name(), "autoFilter" | "sortState" | "sortCondition"));
            if range_attr {
                match rw.moves.sqref(rw.home, a.value()) {
                    Some(v) => moved.insert(a.name(), v),
                    None => return Ok(false),
                };
            } else if a.name() == "id" && name.name() == "brk" {
                let Ok(at) = a.value().parse::<u32>() else {
                    continue;
                };
                let line = if parent == "colBreaks" {
                    rw.moves.cell(rw.home, 0, at).map(|c| c.1)
                } else {
                    rw.moves.cell(rw.home, at, 0).map(|c| c.0)
                };
                match line {
                    Some(v) => moved.insert("id", v.to_string()),
                    None => return Ok(false),
                };
            }
        }
    }
    let mut inner = String::new();
    let mut elements = 0;
    let mut dropped = 0;
    for child in node
        .children()
        .filter(|c| skip.is_none_or(|name| !is_main(*c, name)))
    {
        if write_node_without(child, &mut inner, ids, None, rw)? {
            elements += usize::from(child.is_element());
        } else {
            dropped += 1;
        }
    }
    let list = matches!(
        name.name(),
        "dataValidations" | "rowBreaks" | "colBreaks" | "protectedRanges" | "ignoredErrors"
    );
    if dropped > 0 && elements == 0 && list {
        // A list whose every entry was deleted (e.g. all validations).
        return Ok(false);
    }
    if dropped > 0 && node.attribute("count").is_some() {
        moved.insert("count", elements.to_string());
    }
    out.push('<');
    out.push_str(name.name());
    for a in node.attributes() {
        match moved.get(a.name()).filter(|_| a.namespace().is_none()) {
            Some(v) => out.push_str(&format!(" {}=\"{}\"", a.name(), esc_attr(v))),
            None => write_attr(out, a, ids)?,
        }
    }
    if inner.is_empty() {
        out.push_str("/>");
        return Ok(true);
    }
    out.push('>');
    out.push_str(&inner);
    out.push_str("</");
    out.push_str(name.name());
    out.push('>');
    Ok(true)
}

/// The markup of `node` as written in `src`, with the namespace declarations
/// it inherits added to its start tag, so it can be moved into another
/// document whose default namespace is SpreadsheetML.
fn verbatim(node: Node, src: &str) -> Result<String, String> {
    let range = node.range();
    let text = &src[range.clone()];
    let parent = node.parent();
    let inherited = |ns: &roxmltree::Namespace| {
        parent.is_some_and(|p| {
            p.namespaces()
                .any(|q| q.name() == ns.name() && q.uri() == ns.uri())
        })
    };
    let name_end = text
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .ok_or("broken start tag")?;
    let start_tag = &text[..text.find('>').ok_or("broken start tag")?];
    let mut decls = String::new();
    for ns in node.namespaces().filter(|ns| inherited(ns)) {
        match ns.name() {
            None if ns.uri() == NS_MAIN => {}
            None => return Err("other default namespace".into()),
            Some("xml") => {}
            // Declared here already, or not used inside.
            Some(prefix)
                if start_tag.contains(&format!("xmlns:{prefix}="))
                    || !text.contains(&format!("{prefix}:")) => {}
            Some(prefix) => {
                decls.push_str(&format!(" xmlns:{prefix}=\"{}\"", esc_attr(ns.uri())));
            }
        }
    }
    Ok(format!("{}{decls}{}", &text[..name_end], &text[name_end..]))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SheetKind {
    Work,
    Chart,
}

struct SheetEntry {
    name: String,
    sheet_id: u32,
    part: String,
    kind: SheetKind,
    /// Position among the workbook's `<sheet>` elements (what `localSheetId`
    /// counts).
    pos: usize,
    /// The `<sheet>` element's `state`, if any.
    state: Option<String>,
    /// The relationship type of the sheet part.
    rel_kind: String,
}

/// The workbook part (from `_rels/.rels`) and its worksheets and chart sheets
/// in tab order. Sheets whose part can't be resolved (and dialog sheets) are
/// left out.
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
    for (pos, s) in doc
        .descendants()
        .filter(|n| is_main(*n, "sheet"))
        .enumerate()
    {
        let rid = sheet_rid(s);
        let rel = rid.and_then(|id| wb_rels.iter().find(|r| r.id == id));
        let Some(rel) = rel.filter(|r| !r.external) else {
            continue;
        };
        let kind = match rel.short_kind() {
            "worksheet" => SheetKind::Work,
            "chartsheet" => SheetKind::Chart,
            _ => continue,
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
            kind,
            pos,
            state: s.attribute("state").map(str::to_owned),
            rel_kind: rel.kind.clone(),
        });
    }
    Ok((wb, wb_rels, wb_xml, sheets))
}

/// The worksheets of a package as `(sheetId, xml)`, each with the inside of
/// `<sheetData>` left out ([`Skeleton`]): for readers of markup IronCalc
/// doesn't model (data validation). Sheets that can't be read are skipped.
pub(crate) fn worksheet_skeletons(bytes: &[u8]) -> Result<Vec<(u32, String)>, String> {
    let orig = Original::new(bytes)?;
    let Some(root) = orig.text("_rels/.rels")? else {
        return Ok(Vec::new());
    };
    let root_rels = parse_rels(&root)?;
    let (_, _, _, sheets) = workbook_sheets(&root_rels, &|name| orig.text(name))?;
    Ok(sheets
        .into_iter()
        .filter(|s| s.kind == SheetKind::Work)
        .filter_map(|s| match orig.skeleton(&s.part) {
            Ok(Some(sk)) => Some((s.sheet_id, sk.xml)),
            _ => None,
        })
        .collect())
}

/// A worksheet's drawing part as the original package has it, with the
/// parts it links to (charts and pictures) read, for showing them.
pub(crate) struct DrawingSource {
    pub sheet_id: u32,
    pub xml: String,
    /// The drawing's relationships: id -> (short type, part).
    pub targets: HashMap<String, (String, String)>,
    /// Chart and picture parts the drawing links to, by name.
    pub parts: HashMap<String, Vec<u8>>,
}

/// What [`drawing_sources`] reads from a package.
#[derive(Default)]
pub(crate) struct DrawingSources {
    pub drawings: Vec<DrawingSource>,
    /// `xl/theme/theme1.xml` (charts take their colours and fonts from it).
    pub theme: Option<String>,
    /// Every sheet's name and `sheetId` (references in charts name sheets).
    pub sheets: Vec<(String, u32)>,
    /// The chart sheets, in tab order.
    pub chart_sheets: Vec<ChartSheetSource>,
}

/// A chart sheet of a package (`xl/chartsheets/sheetN.xml`).
pub(crate) struct ChartSheetSource {
    pub name: String,
    /// `state` hidden or very hidden.
    pub hidden: bool,
    /// The `sheetId`s of the worksheets before it, nearest first: saving
    /// puts it back after the first of them still there ([`sheet_slots`]).
    pub before: Vec<u32>,
    pub drawing: Option<DrawingSource>,
    /// The chart sheet part's XML (its print settings).
    pub xml: Option<String>,
}

/// The drawings of the worksheets of `bytes`. Parts that can't be read are
/// skipped (they are still carried on save).
pub(crate) fn drawing_sources(bytes: &[u8]) -> Result<DrawingSources, String> {
    let orig = Original::new(bytes)?;
    let mut out = DrawingSources::default();
    let Some(root) = orig.text("_rels/.rels")? else {
        return Ok(out);
    };
    let root_rels = parse_rels(&root)?;
    let (wb, wb_rels, _, sheets) = workbook_sheets(&root_rels, &|name| orig.text(name))?;
    out.sheets = sheets
        .iter()
        .map(|s| (s.name.clone(), s.sheet_id))
        .collect();
    out.theme = wb_rels
        .iter()
        .find(|r| r.short_kind() == "theme" && !r.external)
        .and_then(|r| resolve(&wb, &r.target))
        .and_then(|t| orig.text(&t).ok().flatten());
    for (i, s) in sheets.iter().enumerate() {
        let drawing = sheet_drawing(&orig, s);
        match s.kind {
            SheetKind::Work => out.drawings.extend(drawing),
            SheetKind::Chart => out.chart_sheets.push(ChartSheetSource {
                name: s.name.clone(),
                hidden: s.state.as_deref().is_some_and(|v| v != "visible"),
                before: sheets[..i]
                    .iter()
                    .rev()
                    .filter(|w| w.kind == SheetKind::Work)
                    .map(|w| w.sheet_id)
                    .collect(),
                drawing,
                xml: orig.text(&s.part).ok().flatten(),
            }),
        }
    }
    Ok(out)
}

/// The drawing of a worksheet or chart sheet (`<drawing r:id>`; a sheet
/// has at most one), with the charts and pictures it links to.
fn sheet_drawing(orig: &Original<'_>, s: &SheetEntry) -> Option<DrawingSource> {
    let sheet_rels = orig.text(&rels_part(&s.part)).ok().flatten()?;
    let sheet_rels = parse_rels(&sheet_rels).ok()?;
    // Its relationship says which, without reading the sheet's cells.
    let part = sheet_rels
        .iter()
        .find(|r| r.short_kind() == "drawing" && !r.external)
        .and_then(|r| resolve(&s.part, &r.target))?;
    let xml = orig.text(&part).ok().flatten()?;
    let rels = match orig.text(&rels_part(&part)).ok().flatten() {
        Some(x) => parse_rels(&x).unwrap_or_default(),
        None => Vec::new(),
    };
    let mut targets = HashMap::new();
    let mut parts = HashMap::new();
    for r in rels.iter().filter(|r| !r.external) {
        let Some(t) = resolve(&part, &r.target) else {
            continue;
        };
        if matches!(r.short_kind(), "chart" | "image")
            && !parts.contains_key(&t)
            && let Ok(Some(data)) = orig.get(&t)
        {
            parts.insert(t.clone(), data);
        }
        targets.insert(r.id.clone(), (r.short_kind().to_owned(), t));
    }
    Some(DrawingSource {
        sheet_id: s.sheet_id,
        xml,
        targets,
        parts,
    })
}

/// The bytes of part `name` of package `bytes`.
pub(crate) fn read_part(bytes: &[u8], name: &str) -> Result<Vec<u8>, String> {
    Original::new(bytes)?
        .get(name)?
        .ok_or_else(|| format!("{name} is missing"))
}

/// A pivot table of a package, with its cache.
pub(crate) struct PivotSource {
    pub sheet_id: u32,
    /// `xl/pivotTables/pivotTable1.xml` and its XML.
    pub part: String,
    pub xml: String,
    /// The cache definition part and its XML.
    pub cache_part: String,
    pub cache_xml: String,
    /// The cache records part, if the cache has one.
    pub records_part: Option<String>,
}

/// The pivot tables of the worksheets of `bytes`. Parts that can't be read
/// are skipped (they are still carried on save).
pub(crate) fn pivot_sources(bytes: &[u8]) -> Result<Vec<PivotSource>, String> {
    let orig = Original::new(bytes)?;
    let mut out = Vec::new();
    if !orig.names.keys().any(|n| n.starts_with("xl/pivottables/")) {
        return Ok(out);
    }
    let Some(root) = orig.text("_rels/.rels")? else {
        return Ok(out);
    };
    let root_rels = parse_rels(&root)?;
    let (_, _, _, sheets) = workbook_sheets(&root_rels, &|name| orig.text(name))?;
    let target_of = |source: &str, kind: &str| -> Option<String> {
        let rels = parse_rels(&orig.text(&rels_part(source)).ok()??).ok()?;
        rels.iter()
            .find(|r| r.short_kind() == kind && !r.external)
            .and_then(|r| resolve(source, &r.target))
    };
    for s in sheets.iter().filter(|s| s.kind == SheetKind::Work) {
        let Some(sheet_rels) = orig.text(&rels_part(&s.part)).ok().flatten() else {
            continue;
        };
        let Ok(sheet_rels) = parse_rels(&sheet_rels) else {
            continue;
        };
        for r in sheet_rels
            .iter()
            .filter(|r| r.short_kind() == "pivotTable" && !r.external)
        {
            let Some(part) = resolve(&s.part, &r.target) else {
                continue;
            };
            let Some(xml) = orig.text(&part).ok().flatten() else {
                continue;
            };
            let Some(cache_part) = target_of(&part, "pivotCacheDefinition") else {
                continue;
            };
            let Some(cache_xml) = orig.text(&cache_part).ok().flatten() else {
                continue;
            };
            let records_part = target_of(&cache_part, "pivotCacheRecords");
            out.push(PivotSource {
                sheet_id: s.sheet_id,
                part,
                xml,
                cache_part,
                cache_xml,
                records_part,
            });
        }
    }
    Ok(out)
}

/// Whether `node` and everything in it is plain SpreadsheetML: no element or
/// attribute in another namespace (which [`write_node`] drops, e.g. `xr:uid`,
/// as IronCalc's sheets don't declare them ignorable).
fn only_main(node: Node) -> bool {
    node.descendants().filter(|n| n.is_element()).all(|n| {
        n.tag_name().namespace() == Some(NS_MAIN) && n.attributes().all(|a| a.namespace().is_none())
    })
}

fn sheet_rid<'a>(s: Node<'a, '_>) -> Option<&'a str> {
    s.attributes()
        .find(|a| matches!(a.namespace(), Some(NS_R) | Some(NS_R_STRICT)) && a.name() == "id")
        .map(|a| a.value())
}

/// A worksheet's XML with the inside of `<sheetData>` left out: all the merge
/// looks at, and cheap to parse however many cells the sheet has.
struct Skeleton {
    xml: String,
    /// `(offset, length)` of the cells left out, in the full XML.
    cut: Option<(usize, usize)>,
}

impl Skeleton {
    fn new(full: &str) -> Skeleton {
        let cut = (|| {
            let open = full
                .match_indices("<sheetData")
                .map(|(i, _)| i)
                .find(|&i| {
                    full[i + "<sheetData".len()..]
                        .starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace())
                })?;
            let gt = open + full[open..].find('>')?;
            if full.as_bytes()[gt - 1] == b'/' {
                return None;
            }
            let close = full.rfind("</sheetData>")?;
            (close > gt).then_some((gt + 1, close - gt - 1))
        })();
        match cut {
            Some((at, len)) => Skeleton {
                xml: format!("{}{}", &full[..at], &full[at + len..]),
                cut,
            },
            None => Skeleton {
                xml: full.to_owned(),
                cut: None,
            },
        }
    }

    /// An offset in [`Skeleton::xml`] as an offset in the full XML.
    fn full_offset(&self, at: usize) -> usize {
        match self.cut {
            Some((cut, len)) if at > cut => at + len,
            _ => at,
        }
    }
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
    /// Parts copied with changes (tables with renamed columns or moved).
    replaced: HashMap<String, Vec<u8>>,
    /// The sheet (`sheetId`) a carried part belongs to, for parts whose
    /// references are relative to it (comments, drawings, tables).
    home: HashMap<String, u32>,
    /// Sheet names of the original (lower case) -> `sheetId`.
    sheets: HashMap<String, u32>,
    moves: &'o Moves,
    /// Drawing objects edited, by `sheetId`.
    drawings: &'o HashMap<u32, Vec<crate::drawing::SaveEdit>>,
    /// Ranges of the saved sheets (full XML) to cut: IronCalc's conditional
    /// formats where the original's are carried.
    removals: HashMap<String, Vec<std::ops::Range<usize>>>,
    /// Conditional formats: some sheet's were carried from the original /
    /// some sheet's could not be (IronCalc's stay, with IronCalc's dxfs).
    cf_carried: bool,
    cf_failed: bool,
    /// Parts not to carry (deleted pivot tables), lower case.
    dropped: HashSet<String>,
}

impl Merge<'_, '_> {
    /// Plan copying `part` and everything it links to. Nothing is planned if
    /// any of it can't be carried.
    fn carry(&mut self, part: &str) -> Result<(), String> {
        self.carry_for(part, None)
    }

    /// [`Merge::carry`], noting that the new parts belong to sheet `home`.
    fn carry_for(&mut self, part: &str, home: Option<u32>) -> Result<(), String> {
        let mut plan = self.copy.clone();
        self.carry_into(part, &mut plan)?;
        if let Some(home) = home {
            for p in plan.difference(&self.copy) {
                self.home.entry(p.clone()).or_insert(home);
            }
        }
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
            self.carry_into(&target, plan)?;
        }
        Ok(())
    }

    /// Copy the planned parts and register their content types. Parts with
    /// cell references follow the rows and columns that moved.
    fn apply(&self, new: &mut Package, notes: &mut Vec<String>) -> Result<(), String> {
        let mut ct_entries = String::new();
        let mut added_defaults: HashSet<String> = HashSet::new();
        for part in &self.copy {
            let data = match self.replaced.get(part) {
                Some(d) => d.clone(),
                None => self
                    .orig
                    .get(part)?
                    .ok_or_else(|| format!("{part} vanished"))?,
            };
            let kind = self
                .orig_ct
                .of(part)
                .ok_or_else(|| format!("{part} has no content type"))?;
            let edited = self
                .home
                .get(part)
                .is_some_and(|h| self.drawings.contains_key(h));
            let data = if (self.moves.is_empty() && !edited) || self.replaced.contains_key(part) {
                data
            } else {
                match move_part(part, kind, &data, self.home.get(part).copied(), self) {
                    Ok(Some(moved)) => moved,
                    Ok(None) => data,
                    Err(e) => {
                        notes.push(format!(
                            "{part}: could not follow the inserted or deleted rows and columns ({e}); kept as it was"
                        ));
                        data
                    }
                }
            };
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

/// Whether `needle` occurs in `hay` (as text when both are UTF-8: the
/// standard library's substring search is much faster than a byte window).
fn memmem(hay: &[u8], needle: &[u8]) -> bool {
    match (std::str::from_utf8(hay), std::str::from_utf8(needle)) {
        (Ok(h), Ok(n)) => h.contains(n),
        _ => hay.windows(needle.len()).any(|w| w == needle),
    }
}

/// Relationships to add to one rels part of the new package.
struct RelAdds {
    source: String,
    rels: Vec<Rel>,
}

/// Carry what IronCalc dropped from `original` into `new`. Returns whether
/// anything was added.
fn merge(
    new: &mut Package,
    original: &[u8],
    ctx: &Context,
    notes: &mut Vec<String>,
) -> Result<bool, String> {
    let orig = Original::new(original)?;
    let orig_ct = ContentTypes::parse(
        &orig
            .text(CONTENT_TYPES)?
            .ok_or("original has no content types")?,
    )?;
    let new_ct = ContentTypes::parse(new.text(CONTENT_TYPES)?.ok_or("no content types")?)?;
    let written: HashSet<String> = new.parts().map(str::to_lowercase).collect();

    // Package level.
    let orig_root = parse_rels(
        &orig
            .text("_rels/.rels")?
            .ok_or("original has no _rels/.rels")?,
    )?;
    let new_root = parse_rels(new.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let orig_read = |p: &str| orig.text(p);
    let (orig_wb, orig_wb_rels, orig_wb_xml, orig_sheets) =
        workbook_sheets(&orig_root, &orig_read)?;
    let new_read = |p: &str| Ok(new.text(p)?.map(str::to_owned));
    let (new_wb, new_wb_rels, new_wb_xml, new_sheets) = workbook_sheets(&new_root, &new_read)?;

    let mut m = Merge {
        orig: &orig,
        orig_ct,
        new_ct,
        copy: BTreeSet::new(),
        written,
        replaced: HashMap::new(),
        home: HashMap::new(),
        sheets: orig_sheets
            .iter()
            .map(|s| (s.name.to_lowercase(), s.sheet_id))
            .collect(),
        moves: ctx.moves,
        drawings: ctx.drawings,
        removals: HashMap::new(),
        cf_carried: false,
        cf_failed: false,
        dropped: HashSet::new(),
    };
    m.dropped = ctx.pivots.dropped.clone();
    m.replaced.extend(
        ctx.pivots
            .replaced
            .iter()
            .filter(|(part, _)| orig.has(part))
            .map(|(part, data)| (part.clone(), data.clone())),
    );
    let mut adds: Vec<RelAdds> = Vec::new();
    let mut sheet_edits: Vec<(String, Vec<Insert>)> = Vec::new();

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

    // Chart sheets (IronCalc has no chart sheets): their parts, and where
    // they go in the new workbook's sheet list.
    let mut wb_ids = IdAlloc::new(&new_wb_rels);
    let charts = carry_chart_sheets(
        &mut m,
        &orig_sheets,
        &new_sheets,
        &new_wb,
        &mut wb_ids,
        notes,
    );
    let chart_rids: BTreeSet<String> = orig_sheets
        .iter()
        .filter(|s| s.kind == SheetKind::Chart)
        .filter_map(|s| {
            orig_wb_rels
                .iter()
                .find(|r| resolve(&orig_wb, &r.target).as_deref() == Some(s.part.as_str()))
                .map(|r| r.id.clone())
        })
        .collect();

    // Workbook level.
    let wb_refs = Document::parse(&orig_wb_xml)
        .map(|d| rel_refs(d.root()))
        .map_err(|e| format!("{orig_wb}: {e}"))?;
    // Chart sheets not kept were reported by `carry_chart_sheets`; pivot
    // caches are carried with the pivot tables that use them
    // ([`carry_pivot_caches`]).
    let mut quiet = chart_rids;
    quiet.extend(
        orig_wb_rels
            .iter()
            .filter(|r| {
                matches!(
                    r.short_kind(),
                    "pivotCacheDefinition" | "slicerCache" | "timelineCache"
                )
            })
            .map(|r| r.id.clone()),
    );
    let mut wb_adds = carry_implicit(
        &mut m,
        Level {
            label: "workbook",
            orig_source: &orig_wb,
            new_source: &new_wb,
            orig_rels: &orig_wb_rels,
            new_rels: &new_wb_rels,
            explicit: &wb_refs,
            quiet: &quiet,
        },
        notes,
    );
    // The chart sheets' ids were taken first; keep the implicit ones unique.
    let mut taken: HashSet<String> = charts.iter().map(|c| c.rel.id.clone()).collect();
    for r in &mut wb_adds.rels {
        while taken.contains(&r.id) {
            r.id = wb_ids.next();
        }
        taken.insert(r.id.clone());
    }
    wb_adds.rels.extend(charts.iter().map(|c| c.rel.clone()));
    adds.push(wb_adds);

    // Sheet level, matched by sheetId.
    let mut ids_seen = HashSet::new();
    let unique_ids = orig_sheets.iter().all(|s| ids_seen.insert(s.sheet_id));
    for os in orig_sheets.iter().filter(|s| s.kind == SheetKind::Work) {
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
        if let Some((add, edit)) = carry_sheet(&mut m, new, os, ns, ctx, notes)? {
            adds.push(add);
            if !edit.is_empty() {
                sheet_edits.push((ns.part.clone(), edit));
            }
        }
    }

    // Pivot caches the carried pivot tables use: workbook relationships and
    // the `<pivotCaches>` list.
    let (cache_rels, pivot_caches) = carry_pivot_caches(
        &m,
        &orig_wb,
        &orig_wb_xml,
        &orig_wb_rels,
        &new_wb,
        &mut wb_ids,
        &taken,
        notes,
    )?;
    if !cache_rels.is_empty() {
        adds.push(RelAdds {
            source: new_wb.clone(),
            rels: cache_rels,
        });
    }
    // Slicer and timeline caches (workbook extensions), with their parts.
    for r in adds
        .iter()
        .filter(|a| a.source == new_wb)
        .flat_map(|a| &a.rels)
    {
        taken.insert(r.id.clone());
    }
    let (ext_rels, wb_exts) = carry_workbook_extensions(
        &mut m,
        &orig_wb,
        &orig_wb_xml,
        &orig_wb_rels,
        &new_wb,
        &mut wb_ids,
        &taken,
        notes,
    )?;
    if !ext_rels.is_empty() {
        adds.push(RelAdds {
            source: new_wb.clone(),
            rels: ext_rels,
        });
    }

    // The workbook part: chart sheets in the sheet list, sheet positions,
    // what defined names had that IronCalc doesn't keep, pivot caches.
    let wb_edit = edit_workbook(
        &new_wb_xml,
        &orig_wb_xml,
        &orig_sheets,
        &new_sheets,
        &charts,
        ctx.moves,
        &m.sheets,
        pivot_caches.as_deref(),
        wb_exts.as_deref(),
        notes,
    )?;

    // The original's dxfs, which the carried conditional formats (and
    // tables) point into, unless IronCalc's own rules stay somewhere.
    let styles_edit = if m.cf_carried && !m.cf_failed {
        match carry_dxfs(&orig, &orig_wb, &orig_wb_rels, new, &new_wb, &new_wb_rels) {
            Ok(edit) => edit,
            Err(e) => {
                notes.push(format!("conditional formats written by Kantor: {e}"));
                return Ok(false);
            }
        }
    } else {
        None
    };

    if m.copy.is_empty()
        && adds.iter().all(|a| a.rels.is_empty())
        && sheet_edits.is_empty()
        && m.removals.is_empty()
        && wb_edit.is_none()
        && styles_edit.is_none()
    {
        return Ok(false);
    }
    m.apply(new, notes)?;
    for add in adds.into_iter().filter(|a| !a.rels.is_empty()) {
        let rp = rels_part(&add.source);
        let mut rels = match new.text(&rp)? {
            Some(x) => parse_rels(x)?,
            None => Vec::new(),
        };
        rels.extend(add.rels);
        new.set(&rp, write_rels(&rels).into_bytes());
    }
    let mut removals = std::mem::take(&mut m.removals);
    for (part, _) in &sheet_edits {
        removals.entry(part.clone()).or_default();
    }
    let mut sheet_edits: HashMap<String, Vec<Insert>> = sheet_edits.into_iter().collect();
    for (part, cuts) in removals {
        // In place: a big sheet is not copied (the edits are small and
        // mostly near its end).
        let mut xml = new.take_text(&part)?.ok_or("sheet vanished")?;
        let edits = sheet_edits.remove(&part).unwrap_or_default();
        // Back to front; at the same offset a cut goes before an insert.
        let mut all: Vec<(usize, usize, usize, Option<String>)> = edits
            .into_iter()
            .map(|(at, order, text)| (at, 1, order, Some(text)))
            .chain(cuts.into_iter().map(|r| (r.start, 0, r.end, None)))
            .collect();
        all.sort_by_key(|(at, kind, order, _)| {
            std::cmp::Reverse((*at, std::cmp::Reverse(*kind), *order))
        });
        for (at, _, end, text) in all {
            match text {
                Some(text) => xml.insert_str(at, &text),
                None => {
                    xml.replace_range(at..end, "");
                }
            }
        }
        new.set(&part, xml.into_bytes());
    }
    if let Some((part, xml)) = styles_edit {
        new.set(&part, xml.into_bytes());
    }
    if let Some(xml) = wb_edit {
        new.set(&new_wb, xml.into_bytes());
    }
    Ok(true)
}

/// The original's pivot caches that carried pivot tables use: the workbook
/// relationships to add and the `<pivotCaches>` element listing them (by
/// their original `cacheId`, which the pivot tables refer to). A cache no
/// carried pivot table uses is dropped.
#[allow(clippy::too_many_arguments)]
fn carry_pivot_caches(
    m: &Merge,
    orig_wb: &str,
    orig_wb_xml: &str,
    orig_wb_rels: &[Rel],
    new_wb: &str,
    ids: &mut IdAlloc,
    taken: &HashSet<String>,
    notes: &mut Vec<String>,
) -> Result<(Vec<Rel>, Option<String>), String> {
    if !orig_wb_xml.contains("pivotCache") {
        return Ok((Vec::new(), None));
    }
    let doc = Document::parse(orig_wb_xml).map_err(|e| format!("{orig_wb}: {e}"))?;
    let Some(list) = doc
        .root_element()
        .children()
        .find(|c| is_main(*c, "pivotCaches"))
    else {
        return Ok((Vec::new(), None));
    };
    let copied: HashSet<String> = m.copy.iter().map(|p| p.to_lowercase()).collect();
    let mut rels = Vec::new();
    let mut text = String::new();
    for c in list.children().filter(|c| is_main(*c, "pivotCache")) {
        let id = c.attribute("cacheId").unwrap_or_default();
        let rid = c
            .attributes()
            .find(|a| matches!(a.namespace(), Some(NS_R) | Some(NS_R_STRICT)) && a.name() == "id")
            .map(|a| a.value());
        let rel = rid.and_then(|rid| orig_wb_rels.iter().find(|r| r.id == rid && !r.external));
        let target = rel.and_then(|r| Some((r, resolve(orig_wb, &r.target)?)));
        let Some((rel, target)) = target else {
            notes.push(format!("pivot cache {id}: broken relationship, not kept"));
            continue;
        };
        if !copied.contains(&target.to_lowercase()) {
            notes.push(format!(
                "pivot cache {id} ({target}) is not kept: no pivot table kept uses it"
            ));
            continue;
        }
        let mut new_id = ids.next();
        while taken.contains(&new_id) {
            new_id = ids.next();
        }
        text.push_str(&format!(
            "<pivotCache cacheId=\"{}\" r:id=\"{}\"/>",
            esc_attr(id),
            esc_attr(&new_id)
        ));
        rels.push(Rel {
            id: new_id,
            kind: rel.kind.clone(),
            target: relative(new_wb, &target),
            external: false,
        });
    }
    if rels.is_empty() {
        return Ok((Vec::new(), None));
    }
    Ok((rels, Some(format!("<pivotCaches>{text}</pivotCaches>"))))
}

/// The original workbook's slicer and timeline cache extensions
/// ([`WORKBOOK_EXTENSIONS`]), with the parts they link to planned for
/// copying: the relationships to add and the `<ext>` elements (ids renamed).
#[allow(clippy::too_many_arguments)]
fn carry_workbook_extensions(
    m: &mut Merge,
    orig_wb: &str,
    orig_wb_xml: &str,
    orig_wb_rels: &[Rel],
    new_wb: &str,
    ids: &mut IdAlloc,
    taken: &HashSet<String>,
    notes: &mut Vec<String>,
) -> Result<(Vec<Rel>, Option<String>), String> {
    if !orig_wb_xml.contains("extLst") {
        return Ok((Vec::new(), None));
    }
    let doc = Document::parse(orig_wb_xml).map_err(|e| format!("{orig_wb}: {e}"))?;
    let Some(list) = doc
        .root_element()
        .children()
        .find(|c| is_main(*c, "extLst"))
    else {
        return Ok((Vec::new(), None));
    };
    let mut rels = Vec::new();
    let mut text = String::new();
    for ext in list.children().filter(|e| is_main(*e, "ext")) {
        let uri = ext.attribute("uri").unwrap_or_default();
        if !WORKBOOK_EXTENSIONS.contains(&uri) {
            continue;
        }
        let (saved_copy, saved_home) = (m.copy.clone(), m.home.clone());
        let planned = (|| {
            let mut map = HashMap::new();
            let mut added = Vec::new();
            for id in rel_refs(ext) {
                let r = orig_wb_rels
                    .iter()
                    .find(|r| r.id == id && !r.external)
                    .ok_or_else(|| format!("relationship {id} is missing"))?;
                let target = resolve(orig_wb, &r.target)
                    .ok_or_else(|| format!("can't follow target {}", r.target))?;
                m.carry(&target)?;
                let mut new_id = ids.next();
                while taken.contains(&new_id) {
                    new_id = ids.next();
                }
                added.push(Rel {
                    id: new_id.clone(),
                    kind: r.kind.clone(),
                    target: relative(new_wb, &target),
                    external: false,
                });
                map.insert(id, new_id);
            }
            let t = rename_rel_ids(&verbatim(ext, orig_wb_xml)?, &map)?;
            Ok::<_, String>((t, added))
        })();
        match planned {
            Ok((t, added)) => {
                text.push_str(&t);
                rels.extend(added);
            }
            Err(e) => {
                (m.copy, m.home) = (saved_copy, saved_home);
                notes.push(format!("workbook: extension {uri} not kept: {e}"));
            }
        }
    }
    Ok((rels, (!text.is_empty()).then_some(text)))
}

/// The saved styles part with the original's `<dxfs>` in place of
/// IronCalc's: `(part, xml)`, or `None` if the original has none.
fn carry_dxfs(
    orig: &Original,
    orig_wb: &str,
    orig_wb_rels: &[Rel],
    new: &Package,
    new_wb: &str,
    new_wb_rels: &[Rel],
) -> Result<Option<(String, String)>, String> {
    let styles = |wb: &str, rels: &[Rel]| {
        rels.iter()
            .find(|r| r.short_kind() == "styles" && !r.external)
            .and_then(|r| resolve(wb, &r.target))
    };
    let Some(orig_part) = styles(orig_wb, orig_wb_rels) else {
        return Ok(None);
    };
    let Some(orig_xml) = orig.text(&orig_part)? else {
        return Ok(None);
    };
    let orig_doc = Document::parse(&orig_xml).map_err(|e| format!("{orig_part}: {e}"))?;
    let Some(dxfs) = orig_doc
        .root_element()
        .children()
        .find(|c| is_main(*c, "dxfs"))
    else {
        return Ok(None);
    };
    let text = verbatim(dxfs, &orig_xml)?;
    let new_part = styles(new_wb, new_wb_rels).ok_or("saved workbook has no styles")?;
    let new_xml = new.text(&new_part)?.ok_or("saved styles are missing")?;
    let new_doc = Document::parse(new_xml).map_err(|e| format!("saved styles: {e}"))?;
    let root = new_doc.root_element();
    if root.lookup_namespace_uri(None) != Some(NS_MAIN) {
        return Err("saved styles use another namespace".into());
    }
    let range = match root.children().find(|c| is_main(*c, "dxfs")) {
        Some(d) => d.range(),
        None => {
            // Before what follows dxfs in CT_Stylesheet, else at the end.
            let at = root
                .children()
                .find(|c| {
                    ["tableStyles", "colors", "extLst"]
                        .iter()
                        .any(|n| is_main(*c, n))
                })
                .map(|c| c.range().start)
                .or_else(|| new_xml[..root.range().end].rfind("</"))
                .ok_or("saved styles are not closed")?;
            at..at
        }
    };
    // Dxfs the model added (rules made in Kantor) come after the
    // original's: append IronCalc's.
    let orig_count = dxfs.children().filter(|c| is_main(*c, "dxf")).count();
    let extra: Vec<Node> = root
        .children()
        .find(|c| is_main(*c, "dxfs"))
        .map(|d| d.children().filter(|c| is_main(*c, "dxf")).collect())
        .unwrap_or_default();
    let text = if extra.len() > orig_count {
        let mut inner = String::new();
        for c in dxfs.children().filter(|c| c.is_element()) {
            inner.push_str(&verbatim(c, &orig_xml)?);
        }
        for c in &extra[orig_count..] {
            inner.push_str(&verbatim(*c, new_xml)?);
        }
        format!("<dxfs count=\"{}\">{inner}</dxfs>", extra.len())
    } else {
        text
    };
    let mut xml = new_xml.to_owned();
    xml.replace_range(range, &text);
    Ok(Some((new_part, xml)))
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
    carry_implicit_for(m, level, None, notes)
}

fn carry_implicit_for(
    m: &mut Merge,
    level: Level,
    home: Option<u32>,
    notes: &mut Vec<String>,
) -> RelAdds {
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
        if m.dropped.contains(&target.to_lowercase()) {
            continue;
        }
        let new_target = relative(new_source, &target);
        if kind == "comments" && !new_target.starts_with("../") {
            // IronCalc's reader assumes comments live in `xl/`.
            notes.push(format!("{label}: comments at {target} are not kept"));
            continue;
        }
        match m.carry_for(&target, home) {
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

/// Plan the relationships an element of the original sheet uses (`refs`):
/// their targets are carried, and new ids allocated. Returns the id map and
/// the relationships to add; on failure nothing stays planned.
fn plan_rels(
    m: &mut Merge,
    refs: BTreeSet<String>,
    orig_rels: &[Rel],
    os: &SheetEntry,
    ns: &SheetEntry,
    ids: &mut IdAlloc,
) -> Result<(HashMap<String, String>, Vec<Rel>), String> {
    let saved_copy = m.copy.clone();
    let saved_home = m.home.clone();
    let result = (|| {
        let mut map = HashMap::new();
        let mut rels = Vec::new();
        for id in refs {
            let r = orig_rels
                .iter()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("relationship {id} is missing"))?;
            if NEVER_CARRY.contains(&r.short_kind()) || r.short_kind() == "hyperlink" {
                return Err(format!("uses a {}", r.short_kind()));
            }
            let new_id = ids.next();
            if r.external {
                rels.push(Rel {
                    id: new_id.clone(),
                    ..r.clone()
                });
            } else {
                let target = resolve(&os.part, &r.target)
                    .ok_or_else(|| format!("can't follow target {}", r.target))?;
                m.carry_for(&target, Some(os.sheet_id))?;
                rels.push(Rel {
                    id: new_id.clone(),
                    kind: r.kind.clone(),
                    target: relative(&ns.part, &target),
                    external: false,
                });
            }
            map.insert(id, new_id);
        }
        Ok((map, rels))
    })();
    if result.is_err() {
        m.copy = saved_copy;
        m.home = saved_home;
    }
    result
}

/// Carry one sheet's relationships and elements.
fn carry_sheet(
    m: &mut Merge,
    new: &Package,
    os: &SheetEntry,
    ns: &SheetEntry,
    ctx: &Context,
    notes: &mut Vec<String>,
) -> Result<Option<SheetCarry>, String> {
    let orig_rels = match m.orig.text(&rels_part(&os.part))? {
        Some(x) => parse_rels(&x).map_err(|e| format!("{}: {e}", os.part))?,
        None => Vec::new(),
    };
    let Some(orig_sk) = m.orig.skeleton(&os.part)? else {
        return Ok(None);
    };
    let might_carry = CARRIED_ELEMENTS.iter().any(|e| orig_sk.xml.contains(e))
        || orig_sk.xml.contains("conditionalFormatting");
    if orig_rels.is_empty() && !might_carry {
        return Ok(None);
    }
    let orig_doc =
        Document::parse(&orig_sk.xml).map_err(|e| format!("sheet {:?}: {e}", os.name))?;
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
    let mut adds = carry_implicit_for(
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
        Some(os.sheet_id),
        notes,
    );

    // Elements IronCalc doesn't write, with the relationships they use.
    let new_sk = Skeleton::new(new.text(&ns.part)?.ok_or("saved sheet is missing")?);
    let new_doc = Document::parse(&new_sk.xml).map_err(|e| format!("saved {label}: {e}"))?;
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
    let close = new_sk.xml[..new_root.range().end]
        .rfind("</")
        .filter(|&i| i > new_root.range().start)
        .ok_or("saved sheet is not closed")?;

    let mut ids = IdAlloc::new(&[new_rels.as_slice(), adds.rels.as_slice()].concat());
    let bindings_ok = new_root.lookup_namespace_uri(None) == Some(NS_MAIN)
        && new_root.lookup_namespace_uri(Some("r")) == Some(NS_R);
    let sheet_ids = m.sheets.clone();
    let rw = Rewrite {
        moves: ctx.moves,
        home: os.sheet_id,
        sheets: &sheet_ids,
    };
    let mut inserts: Vec<Insert> = Vec::new();
    // Conditional formats: the original's markup replaces what IronCalc
    // wrote from its model (which drops attributes such as `reverse` and
    // `showValue`, and rewrites text rules), with the original's dxfs.
    let own_cf = !ctx.patches.cf_sheets.contains(&os.sheet_id)
        && carry_conditional_formats(
            m,
            orig_root,
            new_root,
            &new_sk,
            &ns.part,
            &orig_sk.xml,
            (&present, close, bindings_ok),
            &rw,
            &mut inserts,
            &label,
            notes,
        );
    let mut into_ext = false;
    for child in orig_root.children().filter(|n| n.is_element()) {
        let local = child.tag_name().name();
        if child.tag_name().namespace() != Some(NS_MAIN) || !CARRIED_ELEMENTS.contains(&local) {
            continue;
        }
        let order = SHEET_ORDER
            .iter()
            .position(|e| *e == local)
            .expect("carried elements are in the schema order");
        let existing = present.iter().find(|(i, _)| *i == order).map(|(_, at)| *at);
        if local == "extLst" {
            let at_end = existing.map(|_| {
                let n = new_root
                    .children()
                    .find(|c| is_main(*c, "extLst"))
                    .expect("present");
                n.range().end - "</extLst>".len()
            });
            let mut ext_rels: Vec<Rel> = Vec::new();
            let exts = {
                let mut plan = |refs: BTreeSet<String>| {
                    let (map, rels) = plan_rels(m, refs, &orig_rels, os, ns, &mut ids)?;
                    ext_rels.extend(rels);
                    Ok(map)
                };
                carry_extensions(child, new_root, &orig_sk.xml, &rw, own_cf, &mut plan)
            };
            let exts = match exts {
                Ok(e) => e,
                Err(e) => {
                    notes.push(format!("{label}: <extLst> not kept: {e}"));
                    continue;
                }
            };
            adds.rels.extend(ext_rels);
            for skipped in &exts.skipped {
                notes.push(format!("{label}: extension {skipped} not kept"));
            }
            if exts.text.is_empty() {
                continue;
            }
            match at_end {
                // Into IronCalc's own extLst, before it closes.
                Some(at) if new_sk.xml[at..].starts_with("</extLst>") => {
                    into_ext = true;
                    inserts.push((new_sk.full_offset(at), order, exts.text));
                }
                Some(_) => notes.push(format!("{label}: <extLst> not kept: unexpected markup")),
                None => {
                    let at = insert_point(&present, order, close);
                    inserts.push((
                        new_sk.full_offset(at),
                        order,
                        format!("<extLst>{}</extLst>", exts.text),
                    ));
                    present.push((order, at));
                }
            }
            continue;
        }
        if existing.is_some() {
            continue;
        }
        if !bindings_ok {
            notes.push(format!(
                "{label}: <{local}> not kept: saved sheet uses other namespace prefixes"
            ));
            continue;
        }
        let text = if local == "tableParts" {
            match carry_tables(m, child, &orig_rels, os, ns, &mut ids, ctx, notes) {
                Some((text, rels)) => {
                    adds.rels.extend(rels);
                    text
                }
                None => continue,
            }
        } else if local == "dataValidations" && !ctx.moves.touches(os.sheet_id) && only_main(child)
        {
            // Nothing moved: exactly as the file had it.
            match verbatim(child, &orig_sk.xml) {
                Ok(text) => text,
                Err(e) => {
                    notes.push(format!("{label}: <{local}> not kept: {e}"));
                    continue;
                }
            }
        } else {
            // Plan the relationships this element uses.
            let (saved_copy, saved_home) = (m.copy.clone(), m.home.clone());
            let (map, rels) = match plan_rels(m, rel_refs(child), &orig_rels, os, ns, &mut ids) {
                Ok(planned) => planned,
                Err(why) => {
                    notes.push(format!("{label}: <{local}> not kept: {why}"));
                    continue;
                }
            };
            let mut text = String::new();
            let skip = (local == "sheetPr").then_some("tabColor");
            match write_node_without(child, &mut text, &map, skip, Some(&rw)) {
                Ok(true) => {}
                Ok(false) => {
                    (m.copy, m.home) = (saved_copy, saved_home);
                    notes.push(format!(
                        "{label}: <{local}> dropped: the cells it applied to were deleted"
                    ));
                    continue;
                }
                Err(e) => {
                    (m.copy, m.home) = (saved_copy, saved_home);
                    notes.push(format!("{label}: <{local}> not kept: {e}"));
                    continue;
                }
            }
            adds.rels.extend(rels);
            text
        };
        let at = insert_point(&present, order, close);
        inserts.push((new_sk.full_offset(at), order, text));
        present.push((order, at));
    }
    if own_cf {
        cut_cf_extension(m, new_root, &new_sk, &ns.part, into_ext);
    }
    Ok(Some((adds, inserts)))
}

/// Carry the original sheet's `<conditionalFormatting>` elements in place of
/// IronCalc's (which are cut, see [`Merge::removals`]), with their ranges and
/// formulas following moved rows and columns. Returns whether the original's
/// conditional formats replace IronCalc's (then its x14 extension is carried
/// too, and IronCalc's cut by [`cut_cf_extension`]).
#[allow(clippy::too_many_arguments)]
fn carry_conditional_formats(
    m: &mut Merge,
    orig_root: Node,
    new_root: Node,
    new_sk: &Skeleton,
    new_part: &str,
    src: &str,
    (present, close, bindings_ok): (&[(usize, usize)], usize, bool),
    rw: &Rewrite,
    inserts: &mut Vec<Insert>,
    label: &str,
    notes: &mut Vec<String>,
) -> bool {
    let orig_cf: Vec<Node> = orig_root
        .children()
        .filter(|c| is_main(*c, "conditionalFormatting"))
        .collect();
    let orig_ext = orig_root
        .children()
        .filter(|c| is_main(*c, "extLst"))
        .flat_map(|l| l.children())
        .any(|e| is_main(e, "ext") && e.attribute("uri") == Some(CF_EXTENSION));
    if orig_cf.is_empty() && !orig_ext {
        return false;
    }
    if !bindings_ok {
        notes.push(format!(
            "{label}: conditional formats written by Kantor: saved sheet uses other namespace prefixes"
        ));
        m.cf_failed = true;
        return false;
    }
    let mut text = String::new();
    for cf in &orig_cf {
        match verbatim(*cf, src).and_then(|t| move_cf(&t, rw)) {
            Ok(Some(one)) => text.push_str(&one),
            Ok(None) => notes.push(format!(
                "{label}: a conditional format dropped: the cells it applied to were deleted"
            )),
            Err(e) => {
                notes.push(format!(
                    "{label}: conditional formats written by Kantor: {e}"
                ));
                m.cf_failed = true;
                return false;
            }
        }
    }
    let order = SHEET_ORDER
        .iter()
        .position(|e| *e == "conditionalFormatting")
        .expect("in the schema order");
    let cut: Vec<std::ops::Range<usize>> = new_root
        .children()
        .filter(|c| is_main(*c, "conditionalFormatting"))
        .map(|c| new_sk.full_offset(c.range().start)..new_sk.full_offset(c.range().end))
        .collect();
    m.removals
        .entry(new_part.to_owned())
        .or_default()
        .extend(cut);
    if !text.is_empty() {
        let at = insert_point(present, order, close);
        inserts.push((new_sk.full_offset(at), order, text));
    }
    m.cf_carried = true;
    true
}

/// A `<conditionalFormatting>` (as written by [`verbatim`]) following moved
/// rows and columns: its `sqref`, and its rules' formulas, relative to the
/// top-left of the range ([`Moves::cf_formula`]). `None` if all its cells
/// were deleted.
fn move_cf(xml: &str, rw: &Rewrite) -> Result<Option<String>, String> {
    if !rw.moves.touches(rw.home) {
        return Ok(Some(xml.to_owned()));
    }
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let sqref = root
        .attributes()
        .find(|a| a.name() == "sqref" && a.namespace().is_none())
        .ok_or("conditional format without sqref")?;
    let Some(moved) = rw.moves.sqref(rw.home, sqref.value()) else {
        return Ok(None);
    };
    let mut area: Option<crate::shift::Area> = None;
    for part in sqref.value().split_whitespace() {
        if let Some(r) = parse_range(part) {
            let a = r.area();
            area = Some(match area {
                None => a,
                Some(b) => crate::shift::Area {
                    top: a.top.min(b.top),
                    left: a.left.min(b.left),
                    bottom: a.bottom.max(b.bottom),
                    right: a.right.max(b.right),
                },
            });
        }
    }
    let area = area.ok_or("conditional format with an unreadable sqref")?;
    let mut edits = vec![(sqref.range_value(), esc_attr(&moved))];
    for n in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "formula")
    {
        let Some(t) = n.first_child().filter(|t| t.is_text()) else {
            continue;
        };
        let text = t.text().unwrap_or_default();
        let v = rw.moves.cf_formula(text, rw.home, rw.sheets, area);
        if v != text {
            edits.push((t.range(), esc_text(&v)));
        }
    }
    Ok(Some(apply_edits(xml, edits)))
}

/// Cut IronCalc's x14 conditional formats from the saved sheet: the
/// extension, or its whole `extLst` if nothing else is (or goes) in it.
fn cut_cf_extension(
    m: &mut Merge,
    new_root: Node,
    new_sk: &Skeleton,
    new_part: &str,
    into_ext: bool,
) {
    let Some(list) = new_root.children().find(|c| is_main(*c, "extLst")) else {
        return;
    };
    let exts: Vec<Node> = list.children().filter(|e| e.is_element()).collect();
    let Some(cf) = exts
        .iter()
        .find(|e| is_main(**e, "ext") && e.attribute("uri") == Some(CF_EXTENSION))
    else {
        return;
    };
    let node = if exts.len() == 1 && !into_ext {
        list
    } else {
        *cf
    };
    let range = new_sk.full_offset(node.range().start)..new_sk.full_offset(node.range().end);
    m.removals
        .entry(new_part.to_owned())
        .or_default()
        .push(range);
}

/// Where an element of schema index `order` goes: before the first present
/// element that comes after it, else before the root closes.
fn insert_point(present: &[(usize, usize)], order: usize, close: usize) -> usize {
    present
        .iter()
        .filter(|(i, _)| *i > order)
        .map(|(_, at)| *at)
        .min()
        .unwrap_or(close)
}

// ------------------------------------------------------------------ tables

/// Carry the tables of `<tableParts>` that still fit their sheet: the
/// element to insert and the relationships it uses, or `None` if no table is
/// kept. Tables follow moved rows and columns; a table whose columns were
/// split, or whose header row or every cell was deleted, is dropped. Header
/// cells that were edited rename their column (as Excel does); an empty or
/// duplicate header drops the table (Excel would repair the file).
#[allow(clippy::too_many_arguments)]
fn carry_tables(
    m: &mut Merge,
    parts: Node,
    orig_rels: &[Rel],
    os: &SheetEntry,
    ns: &SheetEntry,
    ids: &mut IdAlloc,
    ctx: &Context,
    notes: &mut Vec<String>,
) -> Option<(String, Vec<Rel>)> {
    let label = format!("sheet {:?}", os.name);
    let mut text = String::new();
    let mut rels = Vec::new();
    let mut kept = 0;
    for tp in parts.children().filter(|c| is_main(*c, "tablePart")) {
        let refs = rel_refs(tp);
        let target = refs.iter().next().and_then(|id| {
            orig_rels
                .iter()
                .find(|r| &r.id == id && !r.external)
                .and_then(|r| resolve(&os.part, &r.target))
        });
        let Some(target) = target else {
            notes.push(format!("{label}: a table not kept: broken relationship"));
            continue;
        };
        let orig_xml = m
            .orig
            .text(&target)
            .and_then(|xml| xml.ok_or_else(|| "missing".to_owned()));
        let name = orig_xml.as_deref().ok().and_then(table_part_name);
        if name
            .as_ref()
            .is_some_and(|n| ctx.patches.removed_tables.contains(n))
        {
            continue;
        }
        let touched = name
            .as_ref()
            .is_some_and(|n| ctx.patches.touched_tables.contains(n));
        let fitted = orig_xml.and_then(|xml| {
            if touched {
                Ok(None)
            } else {
                fit_table(&xml, os.sheet_id, ctx)
            }
        });
        let table = match fitted {
            Ok(t) => t,
            Err(e) => {
                notes.push(format!("{label}: table {target} not kept: {e}"));
                continue;
            }
        };
        if let Some(bytes) = table {
            m.replaced.insert(target.clone(), bytes);
        }
        match plan_rels(m, refs, orig_rels, os, ns, ids) {
            Ok((map, r)) => {
                let mut one = String::new();
                if let Err(e) = write_node(tp, &mut one, &map) {
                    notes.push(format!("{label}: table {target} not kept: {e}"));
                    m.replaced.remove(&target);
                    continue;
                }
                text.push_str(&one);
                rels.extend(r);
                kept += 1;
            }
            Err(e) => {
                m.replaced.remove(&target);
                notes.push(format!("{label}: table {target} not kept: {e}"));
            }
        }
    }
    (kept > 0).then(|| {
        (
            format!("<tableParts count=\"{kept}\">{text}</tableParts>"),
            rels,
        )
    })
}

/// The lower-case `name` of a table part.
fn table_part_name(xml: &str) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let t = doc.root_element();
    t.attribute("name")
        .or_else(|| t.attribute("displayName"))
        .map(str::to_lowercase)
}

/// A table part fitted to the saved sheet: `Ok(None)` if it can be copied as
/// is, `Ok(Some(xml))` if it changed, `Err` if it can't be kept. Its range,
/// columns and filter are the model's (tables grow, shrink and get columns
/// with the rows and columns inserted and deleted, `crate::table`).
fn fit_table(xml: &str, sheet: u32, ctx: &Context) -> Result<Option<Vec<u8>>, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let table = doc.root_element();
    if !is_main(table, "table") || table.lookup_namespace_uri(None) != Some(NS_MAIN) {
        return Err("not a table".into());
    }
    let reference = table.attribute("ref").ok_or("no ref")?;
    let name = table
        .attribute("name")
        .or_else(|| table.attribute("displayName"))
        .ok_or("no name")?;
    let now = (ctx.tables)(name)
        .filter(|t| t.sheet_id == sheet)
        .ok_or("it was removed")?;
    let new_area = parse_range(&now.reference).ok_or("unreadable ref")?.area();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    if reference != now.reference {
        let attr = table
            .attributes()
            .find(|a| a.name() == "ref" && a.namespace().is_none())
            .ok_or("no ref")?;
        edits.push((attr.range_value(), esc_attr(&now.reference)));
    }
    // The filter: the model's, over the table without its total row.
    let filter_ref = {
        let bottom = new_area
            .bottom
            .saturating_sub(now.totals_rows)
            .max(new_area.top);
        format!(
            "{}:{}",
            crate::cell_name(new_area.top, new_area.left),
            crate::cell_name(bottom, new_area.right)
        )
    };
    let new_filter = now
        .auto_filter
        .as_deref()
        .map(|x| ironcalc_base::kantor_tables::with_auto_filter_ref(x, &filter_ref));
    let old_filter = table.children().find(|c| is_main(*c, "autoFilter"));
    match (old_filter, new_filter) {
        (Some(n), Some(x)) => {
            if xml[n.range()] != x {
                edits.push((n.range(), x));
            }
        }
        (Some(n), None) => edits.push((n.range(), String::new())),
        (None, Some(x)) => {
            let at = table
                .children()
                .find(|c| is_main(*c, "sortState") || is_main(*c, "tableColumns"))
                .ok_or("no columns")?
                .range()
                .start;
            edits.push((at..at, x));
        }
        (None, None) => {}
    }
    // A remembered sort no longer describes a moved or resized table.
    if reference != now.reference
        && let Some(n) = table.children().find(|c| is_main(*c, "sortState"))
    {
        edits.push((n.range(), String::new()));
    }
    // Columns: inserted or deleted ones change the list.
    let cols_node = table
        .children()
        .find(|c| is_main(*c, "tableColumns"))
        .ok_or("no columns")?;
    let orig_cols: Vec<Node> = cols_node
        .children()
        .filter(|n| is_main(*n, "tableColumn"))
        .collect();
    let orig_ids: Vec<u32> = orig_cols
        .iter()
        .filter_map(|c| c.attribute("id").and_then(|v| v.parse().ok()))
        .collect();
    let model_ids: Vec<u32> = now.columns.iter().map(|c| c.0).collect();
    let columns_changed = orig_ids != model_ids;
    if columns_changed {
        let mut text = format!("<tableColumns count=\"{}\">", now.columns.len());
        let mut renamed: Vec<(String, String)> = Vec::new();
        for (id, cname) in &now.columns {
            match orig_cols
                .iter()
                .find(|c| c.attribute("id").and_then(|v| v.parse::<u32>().ok()) == Some(*id))
            {
                Some(c) => {
                    let orig_name = c.attribute("name").unwrap_or_default();
                    let mut one = xml[c.range()].to_owned();
                    if orig_name != cname {
                        one = one.replace(
                            &format!("name=\"{}\"", esc_attr(orig_name)),
                            &format!("name=\"{}\"", esc_attr(cname)),
                        );
                        renamed.push((orig_name.to_owned(), cname.clone()));
                    }
                    text.push_str(&one);
                }
                None => text.push_str(&format!(
                    "<tableColumn id=\"{id}\" name=\"{}\"/>",
                    esc_attr(cname)
                )),
            }
        }
        text.push_str("</tableColumns>");
        // References to renamed columns in the columns' formulas.
        for (old, new) in &renamed {
            text = text
                .replace(&format!("[{old}]"), &format!("[{new}]"))
                .replace(&format!("[@{old}]"), &format!("[@{new}]"))
                .replace(&format!(",{old}]"), &format!(",{new}]"));
        }
        edits.push((cols_node.range(), text));
    }
    // Header cells must match the column names.
    let header_rows: u32 = table
        .attribute("headerRowCount")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let columns = orig_cols;
    if now.columns.len() as u32 != new_area.right - new_area.left + 1 {
        return Err("its columns don't match its range".into());
    }
    if header_rows > 0 && columns_changed {
        // Header cells of new columns hold their names (see `Workbook::insert_columns`).
        for (i, (_, cname)) in now.columns.iter().enumerate() {
            let cell = (ctx.cell_text)(sheet, new_area.top, new_area.left + i as u32);
            if cell.trim().is_empty() || cell.to_lowercase() != cname.to_lowercase() {
                return Err(format!("the header of column {cname:?} doesn't match"));
            }
        }
    } else if header_rows > 0 {
        let mut renames: Vec<(String, String)> = Vec::new();
        let mut seen = HashSet::new();
        for (i, col) in columns.iter().enumerate() {
            let name = col.attribute("name").unwrap_or_default();
            let cell = (ctx.cell_text)(sheet, new_area.top, new_area.left + i as u32);
            if !seen.insert(cell.to_lowercase()) {
                return Err(format!("two columns are called {cell:?}"));
            }
            if cell == name {
                continue;
            }
            if cell.trim().is_empty() {
                return Err(format!("the header of column {name:?} is empty"));
            }
            if cell.contains(['[', ']', '\'', '#', '@']) {
                return Err(format!("the new header {cell:?} needs escaping"));
            }
            let attr = col
                .attributes()
                .find(|a| a.name() == "name" && a.namespace().is_none())
                .ok_or("column without name")?;
            edits.push((attr.range_value(), esc_attr(&cell)));
            renames.push((name.to_owned(), cell));
        }
        if !renames.is_empty() {
            for f in table.descendants().filter(|n| {
                is_main(*n, "calculatedColumnFormula") || is_main(*n, "totalsRowFormula")
            }) {
                let Some(t) = f.first_child().filter(|t| t.is_text()) else {
                    continue;
                };
                let mut text = t.text().unwrap_or_default().to_owned();
                for (old, new) in &renames {
                    // `[Units]`, `[@Units]`, `[[#This Row],Units]`.
                    text = text
                        .replace(&format!("[{old}]"), &format!("[{new}]"))
                        .replace(&format!("[@{old}]"), &format!("[@{new}]"))
                        .replace(&format!(",{old}]"), &format!(",{new}]"));
                }
                edits.push((t.range(), esc_text(&text)));
            }
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    Ok(Some(apply_edits(xml, edits).into_bytes()))
}

// ------------------------------------------------------------------ extensions

struct Extensions {
    /// `<ext>` elements to add.
    text: String,
    /// URIs not carried.
    skipped: Vec<String>,
}

/// The `<ext>` children of the original `extLst` to carry: those IronCalc
/// didn't write itself (by URI), without relationship ids, with references
/// moved; sparklines, Excel 2010 data validations and other extensions.
fn carry_extensions(
    orig: Node,
    new_root: Node,
    src: &str,
    rw: &Rewrite,
    own_cf: bool,
    plan: &mut dyn FnMut(BTreeSet<String>) -> Result<HashMap<String, String>, String>,
) -> Result<Extensions, String> {
    let written: HashSet<&str> = new_root
        .children()
        .filter(|c| is_main(*c, "extLst"))
        .flat_map(|l| l.children())
        .filter(|e| is_main(*e, "ext"))
        .filter_map(|e| e.attribute("uri"))
        .collect();
    let mut out = Extensions {
        text: String::new(),
        skipped: Vec::new(),
    };
    for ext in orig.children().filter(|e| is_main(*e, "ext")) {
        let uri = ext.attribute("uri").unwrap_or_default();
        // IronCalc writes x14 conditional formats from its model; they are
        // the original's when its `cfRule`s are carried (`own_cf`).
        if uri == CF_EXTENSION {
            if !own_cf {
                continue;
            }
        } else if written.contains(uri) {
            continue;
        }
        let refs = rel_refs(ext);
        let text = verbatim(ext, src)?;
        let text = if refs.is_empty() {
            text
        } else if LINKED_EXTENSIONS.contains(&uri) {
            // Slicers and timelines: their parts are carried with new ids.
            match plan(refs).and_then(|map| rename_rel_ids(&text, &map)) {
                Ok(t) => t,
                Err(e) => {
                    out.skipped.push(format!("{uri} ({e})"));
                    continue;
                }
            }
        } else {
            out.skipped.push(format!("{uri} (it links to other parts)"));
            continue;
        };
        let text = if rw.moves.is_empty() {
            text
        } else {
            match move_extension(&text, rw) {
                Ok(t) => t,
                Err(e) => {
                    out.skipped.push(format!("{uri} ({e})"));
                    continue;
                }
            }
        };
        out.text.push_str(&text);
    }
    Ok(out)
}

/// An `<ext>` (as written by [`verbatim`]) with its `sqref`s and formulas
/// moved: the item whose cells were all deleted (a sparkline, a validation)
/// is left out.
fn move_extension(xml: &str, rw: &Rewrite) -> Result<String, String> {
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut removed: Vec<std::ops::Range<usize>> = Vec::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        let Some(t) = n.first_child().filter(|t| t.is_text()) else {
            continue;
        };
        if removed
            .iter()
            .any(|r| r.start <= n.range().start && n.range().end <= r.end)
        {
            continue;
        }
        let text = t.text().unwrap_or_default();
        match n.tag_name().name() {
            "sqref" => match rw.moves.sqref(rw.home, text) {
                Some(v) if v != text => edits.push((t.range(), esc_text(&v))),
                Some(_) => {}
                None => {
                    // The item this location belongs to goes.
                    let item = n.parent().ok_or("sqref at the top")?;
                    removed.push(item.range());
                }
            },
            "f" => {
                let v = rw.moves.formula(text, Some(rw.home), rw.sheets);
                if v != text {
                    edits.push((t.range(), esc_text(&v)));
                }
            }
            _ => {}
        }
    }
    edits.retain(|(r, _)| !removed.iter().any(|x| x.start <= r.start && r.end <= x.end));
    edits.extend(removed.into_iter().map(|r| (r, String::new())));
    Ok(apply_edits(xml, edits))
}

// ------------------------------------------------------------------ chart sheets

/// A chart sheet carried into the new workbook.
struct ChartSheet {
    name: String,
    sheet_id: u32,
    state: Option<String>,
    /// The workbook relationship to add.
    rel: Rel,
    /// Position in the original's sheet list.
    orig_index: usize,
}

/// Plan the original's chart sheets (with their drawings and charts) and
/// pick their sheet ids and relationship ids in the new workbook.
fn carry_chart_sheets(
    m: &mut Merge,
    orig_sheets: &[SheetEntry],
    new_sheets: &[SheetEntry],
    new_wb: &str,
    ids: &mut IdAlloc,
    notes: &mut Vec<String>,
) -> Vec<ChartSheet> {
    let mut used_ids: HashSet<u32> = new_sheets.iter().map(|s| s.sheet_id).collect();
    let mut max_id = orig_sheets
        .iter()
        .chain(new_sheets)
        .map(|s| s.sheet_id)
        .max()
        .unwrap_or(0);
    let names: HashSet<String> = new_sheets.iter().map(|s| s.name.to_lowercase()).collect();
    let mut out = Vec::new();
    for (i, cs) in orig_sheets.iter().enumerate() {
        if cs.kind != SheetKind::Chart {
            continue;
        }
        if names.contains(&cs.name.to_lowercase()) {
            notes.push(format!(
                "chart sheet {:?} not kept: a sheet has its name",
                cs.name
            ));
            continue;
        }
        if let Err(e) = m.carry(&cs.part) {
            notes.push(format!("chart sheet {:?} not kept: {e}", cs.name));
            continue;
        }
        let sheet_id = if used_ids.contains(&cs.sheet_id) {
            max_id += 1;
            max_id
        } else {
            cs.sheet_id
        };
        used_ids.insert(sheet_id);
        out.push(ChartSheet {
            name: cs.name.clone(),
            sheet_id,
            state: cs.state.clone(),
            rel: Rel {
                id: ids.next(),
                kind: cs.rel_kind.clone(),
                target: relative(new_wb, &cs.part),
                external: false,
            },
            orig_index: i,
        });
    }
    out
}

/// A sheet of the saved workbook's list.
enum Slot<'a> {
    /// IronCalc's sheet, by its index.
    New(usize),
    Chart(&'a ChartSheet),
}

/// The new workbook's sheets with the chart sheets put back where they were:
/// after the nearest sheet before them that still exists.
fn sheet_slots<'a>(
    orig_sheets: &[SheetEntry],
    new_sheets: &[SheetEntry],
    charts: &'a [ChartSheet],
) -> Vec<Slot<'a>> {
    let mut slots: Vec<Slot> = (0..new_sheets.len()).map(Slot::New).collect();
    for cs in charts {
        let before = orig_sheets[..cs.orig_index]
            .iter()
            .rev()
            .filter(|s| s.kind == SheetKind::Work)
            .find_map(|s| new_sheets.iter().position(|n| n.sheet_id == s.sheet_id));
        let mut at = match before {
            Some(n) => {
                slots
                    .iter()
                    .position(|s| matches!(s, Slot::New(i) if *i == n))
                    .expect("present")
                    + 1
            }
            None => 0,
        };
        // After chart sheets put there before (they came first).
        while matches!(slots.get(at), Some(Slot::Chart(c)) if c.orig_index < cs.orig_index) {
            at += 1;
        }
        slots.insert(at, Slot::Chart(cs));
    }
    slots
}

// ------------------------------------------------------------------ workbook part

/// Defined-name attributes IronCalc doesn't keep, copied from the original.
const NAME_ATTRIBUTES: [&str; 13] = [
    "hidden",
    "comment",
    "customMenu",
    "description",
    "help",
    "statusBar",
    "function",
    "vbProcedure",
    "xlm",
    "functionGroupId",
    "shortcutKey",
    "publishToServer",
    "workbookParameter",
];

/// The new workbook part with chart sheets in its sheet list (and every
/// sheet index that counts them: `localSheetId`, `activeTab`), defined-name
/// attributes IronCalc drops (`hidden` on `_xlnm._FilterDatabase`...), and
/// built-in or hidden names it dropped. `None` if nothing changes.
#[allow(clippy::too_many_arguments)]
fn edit_workbook(
    new_xml: &str,
    orig_xml: &str,
    orig_sheets: &[SheetEntry],
    new_sheets: &[SheetEntry],
    charts: &[ChartSheet],
    moves: &Moves,
    sheet_ids: &HashMap<String, u32>,
    pivot_caches: Option<&str>,
    extensions: Option<&str>,
    notes: &mut Vec<String>,
) -> Result<Option<String>, String> {
    let doc = Document::parse(new_xml).map_err(|e| format!("saved workbook: {e}"))?;
    let root = doc.root_element();
    let orig_doc = Document::parse(orig_xml).map_err(|e| format!("workbook: {e}"))?;
    let slots = sheet_slots(orig_sheets, new_sheets, charts);
    // IronCalc's index -> index in the saved list.
    let new_index: Vec<usize> = (0..new_sheets.len())
        .map(|i| {
            slots
                .iter()
                .position(|s| matches!(s, Slot::New(n) if *n == i))
                .expect("present")
        })
        .collect();
    let index_of_id = |id: u32| -> Option<usize> {
        new_sheets
            .iter()
            .position(|s| s.sheet_id == id)
            .map(|i| new_index[i])
    };
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();

    // The sheet list.
    if !charts.is_empty() {
        let sheets = root
            .children()
            .find(|c| is_main(*c, "sheets"))
            .ok_or("saved workbook has no <sheets>")?;
        let list: Vec<Node> = sheets.children().filter(|c| is_main(*c, "sheet")).collect();
        if list.len() != new_sheets.len() {
            return Err("saved workbook lists other sheets".into());
        }
        let mut text = String::from("<sheets>");
        for slot in &slots {
            match slot {
                Slot::New(i) => text.push_str(&new_xml[list[*i].range()]),
                Slot::Chart(c) => {
                    let state = c
                        .state
                        .as_deref()
                        .map(|s| format!(" state=\"{}\"", esc_attr(s)))
                        .unwrap_or_default();
                    text.push_str(&format!(
                        "<sheet name=\"{}\" sheetId=\"{}\"{state} r:id=\"{}\"/>",
                        esc_attr(&c.name),
                        c.sheet_id,
                        esc_attr(&c.rel.id)
                    ));
                }
            }
        }
        text.push_str("</sheets>");
        edits.push((sheets.range(), text));
        // The selected tab (and the first one shown).
        for view in root.descendants().filter(|n| is_main(*n, "workbookView")) {
            for a in view.attributes().filter(|a| a.namespace().is_none()) {
                if matches!(a.name(), "activeTab" | "firstSheet")
                    && let Some(&i) = a
                        .value()
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| new_index.get(i))
                {
                    edits.push((a.range_value(), i.to_string()));
                }
            }
        }
    }

    // Defined names.
    let orig_ids: Vec<Option<u32>> = {
        let all: Vec<Node> = orig_doc
            .descendants()
            .filter(|n| is_main(*n, "sheet"))
            .collect();
        (0..all.len())
            .map(|pos| {
                orig_sheets
                    .iter()
                    .find(|s| s.pos == pos)
                    .map(|s| s.sheet_id)
            })
            .collect()
    };
    let scope = |n: Node, ids: &dyn Fn(usize) -> Option<u32>| -> Result<Option<u32>, ()> {
        match n.attribute("localSheetId") {
            None => Ok(None),
            Some(v) => v.parse::<usize>().ok().and_then(ids).map(Some).ok_or(()),
        }
    };
    let orig_names: Vec<Node> = orig_doc
        .descendants()
        .filter(|n| is_main(*n, "definedName"))
        .collect();
    let new_list = root.children().find(|c| is_main(*c, "definedNames"));
    let new_names: Vec<Node> = new_list
        .map(|l| {
            l.children()
                .filter(|n| is_main(*n, "definedName"))
                .collect()
        })
        .unwrap_or_default();
    let new_ids = |i: usize| new_sheets.get(i).map(|s| s.sheet_id);
    let orig_id = |i: usize| orig_ids.get(i).copied().flatten();
    let key = |n: Node, sheet: Option<u32>| {
        (
            n.attribute("name").unwrap_or_default().to_lowercase(),
            sheet,
        )
    };
    let mut matched = HashSet::new();
    let mut names_changed = false;
    let mut names_text = String::new();
    for n in &new_names {
        let Ok(sheet) = scope(*n, &new_ids) else {
            names_text.push_str(&new_xml[n.range()]);
            continue;
        };
        let k = key(*n, sheet);
        let orig = orig_names
            .iter()
            .find(|o| scope(**o, &orig_id).ok() == Some(sheet) && key(**o, sheet) == k);
        let mut extra = String::new();
        if let Some(o) = orig {
            matched.insert(o.range().start);
            for a in o.attributes().filter(|a| a.namespace().is_none()) {
                if NAME_ATTRIBUTES.contains(&a.name()) && n.attribute(a.name()).is_none() {
                    extra.push_str(&format!(" {}=\"{}\"", a.name(), esc_attr(a.value())));
                }
            }
        }
        let local = sheet.and_then(index_of_id);
        let local_changed =
            local.is_some_and(|l| n.attribute("localSheetId") != Some(&l.to_string()));
        if extra.is_empty() && !local_changed {
            names_text.push_str(&new_xml[n.range()]);
            continue;
        }
        names_changed = true;
        names_text.push_str(&rewrite_name(*n, new_xml, local, &extra));
    }
    for o in &orig_names {
        if matched.contains(&o.range().start) {
            continue;
        }
        let name = o.attribute("name").unwrap_or_default();
        let builtin = name.starts_with("_xlnm.") || o.attribute("hidden") == Some("1");
        let Ok(sheet) = scope(*o, &orig_id) else {
            continue;
        };
        if !builtin {
            notes.push(format!("defined name {name:?} is not kept"));
            continue;
        }
        let local = match sheet {
            Some(id) => match index_of_id(id) {
                Some(i) => Some(i),
                None => continue, // its sheet is gone
            },
            None => None,
        };
        let formula = o.text().unwrap_or_default();
        let formula = moves.formula(formula, sheet, sheet_ids);
        if formula.contains("#REF!") {
            notes.push(format!(
                "defined name {name:?} dropped: its cells were deleted"
            ));
            continue;
        }
        let mut text = String::from("<definedName");
        for a in o.attributes().filter(|a| a.namespace().is_none()) {
            if a.name() == "localSheetId" {
                continue;
            }
            text.push_str(&format!(" {}=\"{}\"", a.name(), esc_attr(a.value())));
        }
        if let Some(l) = local {
            text.push_str(&format!(" localSheetId=\"{l}\""));
        }
        text.push_str(&format!(">{}</definedName>", esc_text(&formula)));
        names_text.push_str(&text);
        names_changed = true;
    }
    // Text inserted after the sheet list (defined names when IronCalc wrote
    // none, then pivot caches when nothing else goes between).
    let sheets_end = root
        .children()
        .find(|c| is_main(*c, "sheets"))
        .ok_or("saved workbook has no <sheets>")?
        .range()
        .end;
    let mut after_sheets = String::new();
    if names_changed {
        match new_list {
            Some(list) => edits.push((
                list.range(),
                format!("<definedNames>{names_text}</definedNames>"),
            )),
            None => {
                after_sheets.push_str(&format!("<definedNames>{names_text}</definedNames>"));
            }
        }
    }
    if let Some(caches) = pivot_caches {
        if root.children().any(|c| is_main(c, "pivotCaches")) {
            return Err("saved workbook already lists pivot caches".into());
        }
        // CT_Workbook: after these, before smartTagPr, webPublishing...
        let before = [
            "sheets",
            "functionGroups",
            "externalReferences",
            "definedNames",
            "calcPr",
            "oleSize",
            "customWorkbookViews",
        ];
        let last = root
            .children()
            .rfind(|c| before.iter().any(|n| is_main(*c, n)))
            .map_or(sheets_end, |c| c.range().end);
        if last == sheets_end {
            after_sheets.push_str(caches);
        } else {
            edits.push((last..last, caches.to_owned()));
        }
    }
    if !after_sheets.is_empty() {
        edits.push((sheets_end..sheets_end, after_sheets));
    }
    if let Some(exts) = extensions {
        match root.children().find(|c| is_main(*c, "extLst")) {
            Some(list) => {
                let at = list.range().end - "</extLst>".len();
                if !new_xml[at..].starts_with("</extLst>") {
                    return Err("saved workbook's <extLst> is not closed as expected".into());
                }
                edits.push((at..at, exts.to_owned()));
            }
            None => {
                let at = new_xml[..root.range().end]
                    .rfind("</")
                    .ok_or("saved workbook is not closed")?;
                edits.push((at..at, format!("<extLst>{exts}</extLst>")));
            }
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    Ok(Some(apply_edits(new_xml, edits)))
}

/// A `<definedName>` of the saved workbook with its `localSheetId` set to
/// `local` and `extra` attributes added.
fn rewrite_name(n: Node, xml: &str, local: Option<usize>, extra: &str) -> String {
    let mut text = String::from("<definedName");
    for a in n.attributes() {
        if a.namespace().is_none() && a.name() == "localSheetId" {
            continue;
        }
        let _ = write_attr(&mut text, a, &HashMap::new());
    }
    if let Some(l) = local {
        text.push_str(&format!(" localSheetId=\"{l}\""));
    }
    text.push_str(extra);
    text.push('>');
    for c in n.children() {
        text.push_str(&xml[c.range()]);
    }
    text.push_str("</definedName>");
    text
}

// ------------------------------------------------------------------ patches

/// Write the sheet features edited in Kantor (`patches`) over the package.
fn apply_patches(pkg: &mut Package, patches: &Patches) -> Result<(), String> {
    if patches.is_empty() {
        return Ok(());
    }
    let root_rels = parse_rels(pkg.text("_rels/.rels")?.ok_or("no _rels/.rels")?)?;
    let read = |p: &str| Ok(pkg.text(p)?.map(str::to_owned));
    let (wb, _, wb_xml, sheets) = workbook_sheets(&root_rels, &read)?;
    let mut ids: Vec<&u32> = patches.sheets.keys().collect();
    ids.sort();
    for id in ids {
        let patch = &patches.sheets[id];
        let sheet = sheets
            .iter()
            .find(|s| s.sheet_id == *id && s.kind == SheetKind::Work)
            .ok_or_else(|| format!("sheet {id} is missing"))?;
        let xml = pkg.take_text(&sheet.part)?.ok_or("sheet part is missing")?;
        let new = patch_sheet(&xml, patch).map_err(|e| format!("{:?}: {e}", sheet.name))?;
        pkg.set(&sheet.part, new.into_bytes());
    }
    if !patches.names.is_empty() {
        let new = patch_names(&wb_xml, &sheets, &patches.names)?;
        pkg.set(&wb, new.into_bytes());
    }
    if !patches.tables.is_empty() {
        write_tables(pkg, &sheets, &patches.tables)?;
    }
    if !patches.charts.is_empty() || !patches.removed_charts.is_empty() {
        let written = write_charts(pkg, &sheets, &patches.charts, &patches.removed_charts)?;
        *patches.written.borrow_mut() = written;
    }
    for c in &patches.file_charts {
        if let Some(part) = file_chart_part(pkg, &sheets, c.sheet_id, c.cnvpr)? {
            pkg.set(&part, c.xml.clone().into_bytes());
        }
    }
    if !patches.notes.is_empty() {
        write_notes(pkg, &sheets, &patches.notes)?;
    }
    Ok(())
}

const COMMENTS_CT: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml";
const VML_CT: &str = "application/vnd.openxmlformats-officedocument.vmlDrawing";

/// Add the notes of sheets that have no comments part: a `comments` part,
/// a VML drawing with their boxes, and the sheet's `<legacyDrawing>`.
fn write_notes(
    pkg: &mut Package,
    sheets: &[SheetEntry],
    notes: &[(u32, Vec<crate::notes::NoteSave>)],
) -> Result<(), String> {
    let mut ct = String::new();
    let has_vml_default = ContentTypes::parse(pkg.text(CONTENT_TYPES)?.ok_or("no content types")?)?
        .defaults
        .contains_key("vml");
    for (block, (id, list)) in notes.iter().enumerate() {
        let Some(sheet) = sheets
            .iter()
            .find(|s| s.sheet_id == *id && s.kind == SheetKind::Work)
        else {
            continue;
        };
        let rels_name = rels_part(&sheet.part);
        let mut rels = match pkg.text(&rels_name)? {
            Some(x) => parse_rels(x)?,
            None => Vec::new(),
        };
        if rels
            .iter()
            .any(|r| matches!(r.short_kind(), "comments" | "vmlDrawing"))
        {
            continue;
        }
        let comments = (1..)
            .map(|n| format!("xl/comments{n}.xml"))
            .find(|n| !pkg.has(n))
            .expect("a free name");
        let vml = (1..)
            .map(|n| format!("xl/drawings/vmlDrawing{n}.vml"))
            .find(|n| !pkg.has(n))
            .expect("a free name");
        pkg.set(&comments, crate::notes::comments_xml(list).into_bytes());
        pkg.set(&vml, crate::notes::vml_xml(list, block + 1).into_bytes());
        ct.push_str(&format!(
            "<Override PartName=\"/{}\" ContentType=\"{COMMENTS_CT}\"/>",
            esc_attr(&comments)
        ));
        let mut alloc = IdAlloc::new(&rels);
        let vml_rid = alloc.next();
        rels.push(Rel {
            id: alloc.next(),
            kind: format!("{REL_BASE}comments"),
            target: relative(&sheet.part, &comments),
            external: false,
        });
        rels.push(Rel {
            id: vml_rid.clone(),
            kind: format!("{REL_BASE}vmlDrawing"),
            target: relative(&sheet.part, &vml),
            external: false,
        });
        pkg.set(&rels_name, write_rels(&rels).into_bytes());
        let xml = pkg.take_text(&sheet.part)?.ok_or("sheet part is missing")?;
        let patch = SheetPatch {
            elements: vec![(
                "legacyDrawing",
                Some(format!("<legacyDrawing r:id=\"{}\"/>", esc_attr(&vml_rid))),
            )],
            ..SheetPatch::default()
        };
        let new = patch_sheet(&xml, &patch).map_err(|e| format!("{:?}: {e}", sheet.name))?;
        pkg.set(&sheet.part, new.into_bytes());
    }
    if !has_vml_default && !ct.is_empty() {
        ct.push_str(&format!(
            "<Default Extension=\"vml\" ContentType=\"{VML_CT}\"/>"
        ));
    }
    if !ct.is_empty() {
        let mut types = pkg
            .text(CONTENT_TYPES)?
            .ok_or("no content types")?
            .to_owned();
        let at = types.rfind("</Types>").ok_or("content types not closed")?;
        types.insert_str(at, &ct);
        pkg.set(CONTENT_TYPES, types.into_bytes());
    }
    Ok(())
}

/// The chart part of the frame with `cNvPr id` `cnvpr` in the drawing of
/// sheet `sheet_id` (`None` when there is none: the frame was deleted, or
/// the package is IronCalc's own).
fn file_chart_part(
    pkg: &Package,
    sheets: &[SheetEntry],
    sheet_id: u32,
    cnvpr: u32,
) -> Result<Option<String>, String> {
    let Some(sheet) = sheets
        .iter()
        .find(|s| s.sheet_id == sheet_id && s.kind == SheetKind::Work)
    else {
        return Ok(None);
    };
    let rels = match pkg.text(&rels_part(&sheet.part))? {
        Some(x) => parse_rels(x)?,
        None => return Ok(None),
    };
    let Some(xml) = pkg.text(&sheet.part)? else {
        return Ok(None);
    };
    let sk = Skeleton::new(xml);
    let drawing = {
        let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
        doc.root_element()
            .children()
            .find(|c| is_main(*c, "drawing"))
            .and_then(|d| d.attribute((NS_R, "id")))
            .and_then(|rid| rels.iter().find(|r| r.id == rid && !r.external))
            .and_then(|r| resolve(&sheet.part, &r.target))
    };
    let Some(drawing) = drawing else {
        return Ok(None);
    };
    let Some(dxml) = pkg.text(&drawing)? else {
        return Ok(None);
    };
    let doc = Document::parse(dxml).map_err(|e| format!("{drawing}: {e}"))?;
    let id = cnvpr.to_string();
    let rid = doc
        .root_element()
        .children()
        .filter(|a| crate::drawing::is_anchor(*a))
        .find(|a| crate::drawing::c_nv_pr(*a).and_then(|n| n.attribute("id")) == Some(&id))
        .and_then(|a| {
            a.descendants()
                .find(|d| d.tag_name().name() == "chart")
                .and_then(|c| c.attribute((NS_R, "id")))
        });
    let Some(rid) = rid else {
        return Ok(None);
    };
    let drels = match pkg.text(&rels_part(&drawing))? {
        Some(x) => parse_rels(x)?,
        None => return Ok(None),
    };
    Ok(drels
        .iter()
        .find(|r| r.id == rid && !r.external)
        .and_then(|r| resolve(&drawing, &r.target))
        .filter(|p| pkg.has(p)))
}

const CHART_CT: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const DRAWING_CT: &str = "application/vnd.openxmlformats-officedocument.drawing+xml";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";

/// Add charts made in Kantor: each chart part (with its content type), an
/// anchor in the sheet's drawing (a new drawing part, linked from the sheet
/// with `<drawing>`, when it has none) and the drawing's relationship to it.
fn write_charts(
    pkg: &mut Package,
    sheets: &[SheetEntry],
    charts: &[ChartSave],
    removed: &[SavedChart],
) -> Result<Vec<(u32, SavedChart)>, String> {
    let mut written = Vec::new();
    // Charts an earlier save added that are gone now.
    for r in removed {
        if !pkg.has(&r.drawing) {
            continue;
        }
        let dxml = pkg.text(&r.drawing)?.unwrap_or_default().to_owned();
        if let Some(range) = chart_anchor_range(&dxml, r.cnvpr, &r.rid) {
            let mut dxml = dxml;
            dxml.replace_range(range, "");
            pkg.set(&r.drawing, dxml.into_bytes());
            let rels_name = rels_part(&r.drawing);
            if let Some(x) = pkg.text(&rels_name)? {
                let mut rels = parse_rels(x)?;
                rels.retain(|l| l.id != r.rid);
                pkg.set(&rels_name, write_rels(&rels).into_bytes());
            }
            pkg.remove(&r.part);
            pkg.remove(&rels_part(&r.part));
            let types = pkg
                .text(CONTENT_TYPES)?
                .ok_or("no content types")?
                .to_owned();
            let o = format!(
                "<Override PartName=\"/{}\" ContentType=\"{CHART_CT}\"/>",
                esc_attr(&r.part)
            );
            pkg.set(CONTENT_TYPES, types.replace(&o, "").into_bytes());
        }
    }
    let mut ct = String::new();
    let mut by_sheet: Vec<(u32, Vec<&ChartSave>)> = Vec::new();
    for c in charts {
        match by_sheet.iter_mut().find(|(id, _)| *id == c.sheet_id) {
            Some((_, v)) => v.push(c),
            None => by_sheet.push((c.sheet_id, vec![c])),
        }
    }
    for (sheet_id, list) in by_sheet {
        let sheet = sheets
            .iter()
            .find(|s| s.sheet_id == sheet_id && s.kind == SheetKind::Work)
            .ok_or_else(|| format!("chart sheet {sheet_id} is missing"))?;
        let sheet_rels_name = rels_part(&sheet.part);
        let mut sheet_rels = match pkg.text(&sheet_rels_name)? {
            Some(x) => parse_rels(x)?,
            None => Vec::new(),
        };
        // The sheet's drawing: the one its <drawing> points at, if any.
        let xml = pkg
            .text(&sheet.part)?
            .ok_or("sheet part is missing")?
            .to_owned();
        let sk = Skeleton::new(&xml);
        let existing = {
            let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
            let root = doc.root_element();
            root.children()
                .find(|c| is_main(*c, "drawing"))
                .and_then(|d| d.attribute((NS_R, "id")))
                .and_then(|rid| sheet_rels.iter().find(|r| r.id == rid && !r.external))
                .and_then(|r| resolve(&sheet.part, &r.target))
        };
        let drawing = match existing {
            Some(d) if pkg.has(&d) => d,
            _ => {
                let d = (1..)
                    .map(|n| format!("xl/drawings/drawing{n}.xml"))
                    .find(|n| !pkg.has(n))
                    .expect("a free name");
                pkg.set(
                    &d,
                    format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"></xdr:wsDr>"
                    )
                    .into_bytes(),
                );
                ct.push_str(&format!(
                    "<Override PartName=\"/{}\" ContentType=\"{DRAWING_CT}\"/>",
                    esc_attr(&d)
                ));
                let rid = IdAlloc::new(&sheet_rels).next();
                sheet_rels.push(Rel {
                    id: rid.clone(),
                    kind: format!("{REL_BASE}drawing"),
                    target: relative(&sheet.part, &d),
                    external: false,
                });
                let new = put_drawing_element(&xml, &rid)?;
                pkg.set(&sheet.part, new.into_bytes());
                d
            }
        };
        pkg.set(&sheet_rels_name, write_rels(&sheet_rels).into_bytes());
        let drawing_rels_name = rels_part(&drawing);
        let mut drawing_rels = match pkg.text(&drawing_rels_name)? {
            Some(x) => parse_rels(x)?,
            None => Vec::new(),
        };
        let mut dxml = pkg
            .text(&drawing)?
            .ok_or("drawing part is missing")?
            .to_owned();
        let (prefix, mut next_id) = {
            let doc = Document::parse(&dxml).map_err(|e| format!("{drawing}: {e}"))?;
            let root = doc.root_element();
            let prefix = match root.lookup_prefix(NS_XDR) {
                Some("") => String::new(),
                Some(p) => format!("{p}:"),
                None => return Err(format!("{drawing} is not a SpreadsheetML drawing")),
            };
            let max = doc
                .descendants()
                .filter(|n| n.tag_name().name() == "cNvPr")
                .filter_map(|n| n.attribute("id")?.parse::<u32>().ok())
                .max()
                .unwrap_or(1);
            (prefix, max + 1)
        };
        let mut anchors = String::new();
        for c in list {
            // Saved before in this drawing: update it there.
            if let Some(sv) = &c.saved
                && sv.drawing == drawing
                && pkg.has(&sv.part)
                && let Some(range) = chart_anchor_range(&dxml, sv.cnvpr, &sv.rid)
            {
                pkg.set(&sv.part, c.xml.clone().into_bytes());
                dxml.replace_range(range, &chart_anchor(&prefix, c, sv.cnvpr, &sv.rid));
                written.push((c.id, sv.clone()));
                continue;
            }
            let part = (1..)
                .map(|n| format!("xl/charts/chart{n}.xml"))
                .find(|n| !pkg.has(n))
                .expect("a free name");
            pkg.set(&part, c.xml.clone().into_bytes());
            ct.push_str(&format!(
                "<Override PartName=\"/{}\" ContentType=\"{CHART_CT}\"/>",
                esc_attr(&part)
            ));
            let rid = IdAlloc::new(&drawing_rels).next();
            drawing_rels.push(Rel {
                id: rid.clone(),
                kind: format!("{REL_BASE}chart"),
                target: relative(&drawing, &part),
                external: false,
            });
            anchors.push_str(&chart_anchor(&prefix, c, next_id, &rid));
            written.push((
                c.id,
                SavedChart {
                    drawing: drawing.clone(),
                    cnvpr: next_id,
                    part: part.clone(),
                    rid: rid.clone(),
                },
            ));
            next_id += 1;
        }
        let close = dxml
            .rfind("</")
            .ok_or_else(|| format!("{drawing} is not closed"))?;
        if dxml[..].trim_end().ends_with("/>") && !dxml.contains("</") {
            return Err(format!("{drawing} has an empty root"));
        }
        dxml.insert_str(close, &anchors);
        pkg.set(&drawing, dxml.into_bytes());
        pkg.set(&drawing_rels_name, write_rels(&drawing_rels).into_bytes());
    }
    if !ct.is_empty() {
        let mut types = pkg
            .text(CONTENT_TYPES)?
            .ok_or("no content types")?
            .to_owned();
        let at = types.rfind("</Types>").ok_or("content types not closed")?;
        types.insert_str(at, &ct);
        pkg.set(CONTENT_TYPES, types.into_bytes());
    }
    Ok(written)
}

/// The byte range of the anchor holding chart `rid` with `cNvPr id` `cnvpr`
/// in drawing XML.
fn chart_anchor_range(xml: &str, cnvpr: u32, rid: &str) -> Option<std::ops::Range<usize>> {
    let doc = Document::parse(xml).ok()?;
    let a = doc.root_element().children().find(|a| {
        crate::drawing::is_anchor(*a)
            && crate::drawing::c_nv_pr(*a).and_then(|n| n.attribute("id"))
                == Some(&cnvpr.to_string())
            && a.descendants()
                .any(|d| d.tag_name().name() == "chart" && d.attribute((NS_R, "id")) == Some(rid))
    })?;
    Some(a.range())
}

/// The anchor of a new chart in a drawing whose SpreadsheetDrawing prefix is
/// `p` (`xdr:` or empty).
fn chart_anchor(p: &str, c: &ChartSave, id: u32, rid: &str) -> String {
    let (open, close) = match c.anchor {
        crate::drawing::Anchor::TwoCell { edit_as, .. } => {
            let e = match edit_as {
                crate::drawing::EditAs::TwoCell => "",
                crate::drawing::EditAs::OneCell => " editAs=\"oneCell\"",
                crate::drawing::EditAs::Absolute => " editAs=\"absolute\"",
            };
            (
                format!("<{p}twoCellAnchor{e}>"),
                format!("</{p}twoCellAnchor>"),
            )
        }
        crate::drawing::Anchor::OneCell { .. } => (
            format!("<{p}oneCellAnchor>"),
            format!("</{p}oneCellAnchor>"),
        ),
        crate::drawing::Anchor::Absolute { .. } => (
            format!("<{p}absoluteAnchor>"),
            format!("</{p}absoluteAnchor>"),
        ),
    };
    format!(
        "{open}{pos}<{p}graphicFrame macro=\"\"><{p}nvGraphicFramePr><{p}cNvPr id=\"{id}\" name=\"{name}\"/><{p}cNvGraphicFramePr/></{p}nvGraphicFramePr><{p}xfrm><a:off x=\"0\" y=\"0\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"/><a:ext cx=\"0\" cy=\"0\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"/></{p}xfrm><a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"{NS_R}\" r:id=\"{rid}\"/></a:graphicData></a:graphic></{p}graphicFrame><{p}clientData/>{close}",
        pos = c.anchor.position_xml(p),
        name = esc_attr(&c.name),
        rid = esc_attr(rid),
    )
}

/// A worksheet's XML with `<drawing r:id="rid"/>` in schema order.
fn put_drawing_element(full: &str, rid: &str) -> Result<String, String> {
    let sk = Skeleton::new(full);
    let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let prefix = match root.lookup_prefix(NS_R) {
        Some(p) if !p.is_empty() => p.to_owned(),
        _ => return Err("the worksheet doesn't bind the relationships namespace".into()),
    };
    let close = sk.xml[..root.range().end]
        .rfind("</")
        .ok_or("worksheet is not closed")?;
    let order = |name: &str| SHEET_ORDER.iter().position(|n| *n == name);
    let present: Vec<(usize, usize)> = root
        .children()
        .filter(|c| c.is_element())
        .filter_map(|c| Some((order(c.tag_name().name())?, c.range().start)))
        .collect();
    let at = insert_point(&present, order("drawing").expect("known"), close);
    let at = sk.full_offset(at);
    Ok(apply_edits(
        full,
        vec![(
            at..at,
            format!("<drawing {prefix}:id=\"{}\"/>", esc_attr(rid)),
        )],
    ))
}

const TABLE_CT: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml";

/// Write tables made or changed in Kantor: rewrite the package's part of a
/// table with one of its names, or add a part (with its relationship, a
/// `<tablePart>` and a content type).
fn write_tables(
    pkg: &mut Package,
    sheets: &[SheetEntry],
    tables: &[TableSave],
) -> Result<(), String> {
    // Existing table parts: lower-case name -> (part, id).
    let mut existing: HashMap<String, (String, u32)> = HashMap::new();
    let mut max_id = 0;
    let names: Vec<String> = pkg
        .parts()
        .filter(|n| {
            let l = n.to_lowercase();
            l.starts_with("xl/tables/") && l.ends_with(".xml") && !l.contains("/_rels/")
        })
        .map(str::to_owned)
        .collect();
    for part in names {
        let Some(xml) = pkg.text(&part)? else {
            continue;
        };
        let Ok(doc) = Document::parse(xml) else {
            continue;
        };
        let root = doc.root_element();
        let id = root
            .attribute("id")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        max_id = max_id.max(id);
        if let Some(n) = root.attribute("name") {
            existing.insert(n.to_lowercase(), (part.clone(), id));
        }
    }
    let mut ct = String::new();
    for t in tables {
        let found = t
            .names
            .iter()
            .find_map(|n| existing.get(&n.to_lowercase()).cloned());
        if let Some((part, id)) = found {
            let old = pkg.text(&part)?.map(str::to_owned);
            let xml = crate::table_edit::table_xml(&t.table, id, old.as_deref());
            pkg.set(&part, xml.into_bytes());
            continue;
        }
        let sheet = sheets
            .iter()
            .find(|s| s.sheet_id == t.sheet_id && s.kind == SheetKind::Work)
            .ok_or_else(|| format!("table {}: sheet {} is missing", t.table.name, t.sheet_id))?;
        max_id += 1;
        let part = (1..)
            .map(|n| format!("xl/tables/table{n}.xml"))
            .find(|n| !pkg.has(n))
            .expect("a free name");
        pkg.set(
            &part,
            crate::table_edit::table_xml(&t.table, max_id, None).into_bytes(),
        );
        ct.push_str(&format!(
            "<Override PartName=\"/{}\" ContentType=\"{TABLE_CT}\"/>",
            esc_attr(&part)
        ));
        let rels_name = rels_part(&sheet.part);
        let mut rels = match pkg.text(&rels_name)? {
            Some(x) => parse_rels(x)?,
            None => Vec::new(),
        };
        let rid = IdAlloc::new(&rels).next();
        rels.push(Rel {
            id: rid.clone(),
            kind: format!("{REL_BASE}table"),
            target: relative(&sheet.part, &part),
            external: false,
        });
        pkg.set(&rels_name, write_rels(&rels).into_bytes());
        let xml = pkg.take_text(&sheet.part)?.ok_or("sheet part is missing")?;
        let new = add_table_part(&xml, &rid).map_err(|e| format!("{:?}: {e}", sheet.name))?;
        pkg.set(&sheet.part, new.into_bytes());
        existing.insert(t.table.name.to_lowercase(), (part, max_id));
    }
    if !ct.is_empty() {
        let mut types = pkg
            .text(CONTENT_TYPES)?
            .ok_or("no content types")?
            .to_owned();
        let at = types.rfind("</Types>").ok_or("content types not closed")?;
        types.insert_str(at, &ct);
        pkg.set(CONTENT_TYPES, types.into_bytes());
    }
    Ok(())
}

/// A worksheet's XML with `<tablePart r:id="rid"/>` added to its
/// `<tableParts>` (made if missing).
fn add_table_part(full: &str, rid: &str) -> Result<String, String> {
    let sk = Skeleton::new(full);
    let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let prefix = match root.lookup_prefix(NS_R) {
        Some(p) if !p.is_empty() => p.to_owned(),
        _ => return Err("the worksheet doesn't bind the relationships namespace".into()),
    };
    let one = format!("<tablePart {prefix}:id=\"{}\"/>", esc_attr(rid));
    let edit = match root.children().find(|c| is_main(*c, "tableParts")) {
        Some(tp) => {
            let n = tp.children().filter(|c| is_main(*c, "tablePart")).count() + 1;
            let mut text = format!("<tableParts count=\"{n}\">");
            for c in tp.children().filter(|c| c.is_element()) {
                text.push_str(&sk.xml[c.range()]);
            }
            text.push_str(&one);
            text.push_str("</tableParts>");
            (tp.range(), text)
        }
        None => {
            let close = sk.xml[..root.range().end]
                .rfind("</")
                .ok_or("worksheet is not closed")?;
            let at = match root.children().find(|c| is_main(*c, "extLst")) {
                Some(e) => e.range().start,
                None => close,
            };
            (
                at..at,
                format!("<tableParts count=\"1\">{one}</tableParts>"),
            )
        }
    };
    let r = sk.full_offset(edit.0.start)..sk.full_offset(edit.0.end);
    Ok(apply_edits(full, vec![(r, edit.1)]))
}

/// A worksheet's XML with `patch` applied. Only the skeleton is parsed.
fn patch_sheet(full: &str, patch: &SheetPatch) -> Result<String, String> {
    let sk = Skeleton::new(full);
    let doc = Document::parse(&sk.xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.lookup_namespace_uri(None) != Some(NS_MAIN) {
        return Err("unexpected namespaces".into());
    }
    let close = sk.xml[..root.range().end]
        .rfind("</")
        .ok_or("worksheet is not closed")?;
    let order = |name: &str| SHEET_ORDER.iter().position(|n| *n == name);
    let present: Vec<(usize, usize)> = root
        .children()
        .filter(|c| c.is_element())
        .filter_map(|c| Some((order(c.tag_name().name())?, c.range().start)))
        .collect();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut inserts: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    let mut put = |name: &str, text: Option<String>, edits: &mut Vec<_>| -> Result<(), String> {
        let o = order(name).ok_or_else(|| format!("unknown element {name}"))?;
        match root.children().find(|c| is_main(*c, name)) {
            Some(n) => edits.push((n.range(), text.unwrap_or_default())),
            None => {
                if let Some(text) = text {
                    let at = insert_point(&present, o, close);
                    inserts.entry(at).or_default().push((o, text));
                }
            }
        }
        Ok(())
    };
    for (name, text) in &patch.elements {
        put(name, text.clone(), &mut edits)?;
    }
    if let Some(attrs) = &patch.page_setup {
        let text = match root.children().find(|c| is_main(*c, "pageSetup")) {
            Some(n) => {
                let mut out = String::from("<pageSetup");
                for (k, v) in attrs {
                    out.push_str(&format!(" {k}=\"{}\"", esc_attr(v)));
                }
                for a in n.attributes() {
                    let modelled = a.namespace().is_none()
                        && crate::print::PAGE_SETUP_ATTRS.contains(&a.name());
                    if !modelled {
                        write_attr_same(&mut out, a, n)?;
                    }
                }
                out.push_str("/>");
                out
            }
            None => {
                let mut out = String::from("<pageSetup");
                for (k, v) in attrs {
                    out.push_str(&format!(" {k}=\"{}\"", esc_attr(v)));
                }
                out.push_str("/>");
                out
            }
        };
        put("pageSetup", Some(text), &mut edits)?;
    }
    if let Some(fit) = patch.fit_to_page {
        let pr = root.children().find(|c| is_main(*c, "sheetPr"));
        let setup = pr.and_then(|p| p.children().find(|c| is_main(*c, "pageSetUpPr")));
        let new_setup = |old: Option<Node>| -> Result<String, String> {
            let mut out = String::from("<pageSetUpPr");
            if let Some(o) = old {
                for a in o.attributes() {
                    if !(a.namespace().is_none() && a.name() == "fitToPage") {
                        write_attr_same(&mut out, a, o)?;
                    }
                }
            }
            if fit {
                out.push_str(" fitToPage=\"1\"");
            }
            out.push_str("/>");
            Ok(out)
        };
        match (pr, setup) {
            (_, Some(s)) => edits.push((s.range(), new_setup(Some(s))?)),
            (Some(p), None) if fit => {
                // CT_SheetPr: tabColor, outlinePr, pageSetUpPr.
                let at = match p.children().rfind(|c| c.is_element()) {
                    Some(last) => last.range().end,
                    None => {
                        let r = p.range();
                        if sk.xml[..r.end].ends_with("/>") {
                            // `<sheetPr .../>`: open it up.
                            let head = &sk.xml[r.start..r.end - 2];
                            edits.push((
                                r.clone(),
                                format!("{head}>{}</sheetPr>", new_setup(None)?),
                            ));
                            usize::MAX
                        } else {
                            r.end - "</sheetPr>".len()
                        }
                    }
                };
                if at != usize::MAX {
                    edits.push((at..at, new_setup(None)?));
                }
            }
            (None, None) if fit => {
                let first = root
                    .children()
                    .find(|n| n.is_element())
                    .ok_or("empty worksheet")?
                    .range()
                    .start;
                edits.push((
                    first..first,
                    format!("<sheetPr>{}</sheetPr>", new_setup(None)?),
                ));
            }
            _ => {}
        }
    }
    if let Some((origin, split)) = patch.frozen_view
        && let Some(view) = root
            .children()
            .find(|c| is_main(*c, "sheetViews"))
            .and_then(|v| v.children().find(|c| is_main(*c, "sheetView")))
    {
        let mut out = String::from("<sheetView");
        for a in view.attributes() {
            if !(a.namespace().is_none() && a.name() == "topLeftCell") {
                write_attr_same(&mut out, a, view)?;
            }
        }
        out.push_str(&format!(
            " topLeftCell=\"{}\">{}</sheetView>",
            crate::cell_name(origin.0, origin.1),
            frozen_pane_xml(origin, split)
        ));
        edits.push((view.range(), out));
    }
    if !patch.remove_exts.is_empty()
        && let Some(list) = root.children().find(|c| is_main(*c, "extLst"))
    {
        let exts: Vec<Node> = list.children().filter(|e| e.is_element()).collect();
        let gone: Vec<Node> = exts
            .iter()
            .copied()
            .filter(|e| {
                is_main(*e, "ext")
                    && e.attribute("uri").is_some_and(|u| {
                        patch.remove_exts.iter().any(|r| r.eq_ignore_ascii_case(u))
                    })
            })
            .collect();
        if !gone.is_empty() && gone.len() == exts.len() {
            edits.push((list.range(), String::new()));
        } else {
            for e in gone {
                edits.push((e.range(), String::new()));
            }
        }
    }
    for (at, mut list) in inserts {
        list.sort_by_key(|(o, _)| *o);
        edits.push((at..at, list.into_iter().map(|(_, t)| t).collect()));
    }
    // Back to the full XML (cells are never edited).
    let edits = edits
        .into_iter()
        .map(|(r, t)| (sk.full_offset(r.start)..sk.full_offset(r.end), t))
        .collect();
    Ok(apply_edits(full, edits))
}

/// Write attribute `a` of `n` with the prefix its document gives it.
/// `<pane>` and `<selection>`s of frozen panes whose top-left cell is
/// `origin` and whose first unfrozen cell is `split` (both from A1).
fn frozen_pane_xml(origin: (u32, u32), split: (u32, u32)) -> String {
    let rows = split.0.saturating_sub(origin.0);
    let cols = split.1.saturating_sub(origin.1);
    let first = crate::cell_name(split.0, split.1);
    match (rows > 0, cols > 0) {
        (true, true) => format!(
            "<pane xSplit=\"{cols}\" ySplit=\"{rows}\" topLeftCell=\"{first}\" activePane=\"bottomRight\" state=\"frozen\"/>\
             <selection pane=\"topRight\"/><selection pane=\"bottomLeft\"/>\
             <selection pane=\"bottomRight\" activeCell=\"{first}\" sqref=\"{first}\"/>"
        ),
        (true, false) => format!(
            "<pane ySplit=\"{rows}\" topLeftCell=\"{first}\" activePane=\"bottomLeft\" state=\"frozen\"/>\
             <selection pane=\"bottomLeft\" activeCell=\"{first}\" sqref=\"{first}\"/>"
        ),
        (false, true) => format!(
            "<pane xSplit=\"{cols}\" topLeftCell=\"{first}\" activePane=\"topRight\" state=\"frozen\"/>\
             <selection pane=\"topRight\" activeCell=\"{first}\" sqref=\"{first}\"/>"
        ),
        (false, false) => String::new(),
    }
}

fn write_attr_same(out: &mut String, a: roxmltree::Attribute, n: Node) -> Result<(), String> {
    let prefix = match a.namespace() {
        None => String::new(),
        Some(NS_XML) => "xml:".into(),
        Some(ns) => match n.lookup_prefix(ns) {
            Some(p) if !p.is_empty() => format!("{p}:"),
            _ => return Err(format!("attribute {} has no prefix", a.name())),
        },
    };
    out.push_str(&format!(
        " {prefix}{}=\"{}\"",
        a.name(),
        esc_attr(a.value())
    ));
    Ok(())
}

/// The workbook XML with built-in names set or removed per sheet.
fn patch_names(
    xml: &str,
    sheets: &[SheetEntry],
    names: &[(u32, &'static str, Option<String>)],
) -> Result<String, String> {
    let doc = Document::parse(xml).map_err(|e| format!("workbook: {e}"))?;
    let root = doc.root_element();
    let list = root.children().find(|c| is_main(*c, "definedNames"));
    let local_of = |id: u32| sheets.iter().find(|s| s.sheet_id == id).map(|s| s.pos);
    let mut text = String::new();
    // Names kept: those not patched.
    if let Some(l) = list {
        for n in l.children().filter(|c| is_main(*c, "definedName")) {
            let name = n.attribute("name").unwrap_or_default();
            let local = n
                .attribute("localSheetId")
                .and_then(|v| v.parse::<usize>().ok());
            let patched = names.iter().any(|(id, nm, _)| {
                nm.eq_ignore_ascii_case(name) && local.is_some() && local_of(*id) == local
            });
            if !patched {
                text.push_str(&xml[n.range()]);
            }
        }
    }
    for (id, name, formula) in names {
        let (Some(local), Some(f)) = (local_of(*id), formula) else {
            continue;
        };
        text.push_str(&format!(
            "<definedName name=\"{name}\" localSheetId=\"{local}\">{}</definedName>",
            esc_text(f)
        ));
    }
    let new_list = if text.is_empty() {
        String::new()
    } else {
        format!("<definedNames>{text}</definedNames>")
    };
    let edit = match list {
        Some(l) => (l.range(), new_list),
        None => {
            if new_list.is_empty() {
                return Ok(xml.to_owned());
            }
            // CT_Workbook: after sheets, functionGroups, externalReferences.
            let at = root
                .children()
                .rfind(|c| {
                    ["sheets", "functionGroups", "externalReferences"]
                        .iter()
                        .any(|n| is_main(*c, n))
                })
                .ok_or("the workbook has no <sheets>")?
                .range()
                .end;
            (at..at, new_list)
        }
    };
    Ok(apply_edits(xml, vec![edit]))
}

// ------------------------------------------------------------------ moving parts

/// The text node of `parent`'s child called `name` (any namespace), and the
/// number it holds.
fn child_number<'a, 'i>(parent: Node<'a, 'i>, name: &str) -> Option<(Node<'a, 'i>, u32)> {
    let t = text_of(parent.children().find(|c| c.tag_name().name() == name)?)?;
    Some((t, t.text()?.trim().parse().ok()?))
}

/// The text node of an element with only text in it.
fn text_of<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    n.first_child().filter(|t| t.is_text())
}

/// A carried part with its cell references moved, or `None` if nothing in
/// it changes. `home` is the sheet the part belongs to.
fn move_part(
    part: &str,
    kind: &str,
    data: &[u8],
    home: Option<u32>,
    m: &Merge,
) -> Result<Option<Vec<u8>>, String> {
    let moves = m.moves;
    let what = kind.rsplit('.').next().unwrap_or_default();
    let is = |suffix: &str| kind.ends_with(suffix);
    let home_moved = home.is_some_and(|h| moves.touches(h));
    let edited = home.and_then(|h| m.drawings.get(&h));
    let kind_of = if is("spreadsheetml.comments+xml") {
        "comments"
    } else if what == "threadedcomments+xml" {
        "threaded"
    } else if is("officedocument.vmlDrawing") || part.to_lowercase().ends_with(".vml") {
        "vml"
    } else if is("officedocument.drawing+xml") {
        "drawing"
    } else if is("drawingml.chart+xml") {
        "chart"
    } else if is("spreadsheetml.pivotTable+xml") {
        "pivot"
    } else if is("spreadsheetml.pivotCacheDefinition+xml") {
        "pivotCache"
    } else {
        return Ok(None);
    };
    if !matches!(kind_of, "chart" | "pivotCache")
        && !home_moved
        && !(kind_of == "drawing" && edited.is_some())
    {
        return Ok(None);
    }
    let xml = std::str::from_utf8(strip_bom(data)).map_err(|_| "not UTF-8".to_owned())?;
    let doc = Document::parse(xml).map_err(|e| e.to_string())?;
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let home = home.unwrap_or(u32::MAX);
    match kind_of {
        "comments" | "threaded" => {
            let tag = if kind_of == "comments" {
                "comment"
            } else {
                "threadedComment"
            };
            for c in doc
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == tag)
            {
                let Some(a) = c.attributes().find(|a| a.name() == "ref") else {
                    continue;
                };
                match moves.sqref(home, a.value()) {
                    Some(v) if v != a.value() => edits.push((a.range_value(), esc_attr(&v))),
                    Some(_) => {}
                    None => edits.push((c.range(), String::new())),
                }
            }
        }
        "vml" => {
            for data in doc
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "ClientData")
            {
                if let (Some((rt, row)), Some((ct, col))) =
                    (child_number(data, "Row"), child_number(data, "Column"))
                {
                    match moves.cell(home, row, col) {
                        Some((r, c)) => {
                            if r != row {
                                edits.push((rt.range(), r.to_string()));
                            }
                            if c != col {
                                edits.push((ct.range(), c.to_string()));
                            }
                        }
                        None => {
                            // The comment's cell is gone: so is its box.
                            let shape = data.parent().ok_or("ClientData at the top")?;
                            edits.push((shape.range(), String::new()));
                            continue;
                        }
                    }
                }
                let anchor = data.children().find(|c| c.tag_name().name() == "Anchor");
                if let Some(t) = anchor.and_then(text_of) {
                    let v: Vec<u32> = t
                        .text()
                        .unwrap_or_default()
                        .split(',')
                        .filter_map(|x| x.trim().parse().ok())
                        .collect();
                    if v.len() == 8 {
                        let (r1, c1) = moves.point(home, v[2], v[0]);
                        let (r2, c2) = moves.point(home, v[6], v[4]);
                        let anchor = format!(
                            "{c1}, {}, {r1}, {}, {c2}, {}, {r2}, {}",
                            v[1], v[3], v[5], v[7]
                        );
                        edits.push((t.range(), anchor));
                    }
                }
            }
        }
        "drawing" => {
            let edited = edited.map(Vec::as_slice).unwrap_or_default();
            let root = doc.root_element();
            let mut seen = HashSet::new();
            for anchor in root.children().filter(|n| crate::drawing::is_anchor(*n)) {
                let id = crate::drawing::c_nv_pr(anchor)
                    .and_then(|n| n.attribute("id")?.parse::<u32>().ok());
                if let Some(e) = id.and_then(|id| edited.iter().find(|e| e.id == id)) {
                    seen.insert(e.id);
                    let text = match e.anchor {
                        Some(a) => placed_anchor(anchor, &a, xml),
                        None => String::new(),
                    };
                    edits.push((anchor.range(), text));
                    continue;
                }
                for corner in anchor
                    .children()
                    .filter(|n| n.is_element() && matches!(n.tag_name().name(), "from" | "to"))
                {
                    let (Some((rt, row)), Some((ct, col))) =
                        (child_number(corner, "row"), child_number(corner, "col"))
                    else {
                        continue;
                    };
                    let (r, c) = moves.point(home, row, col);
                    if r != row {
                        edits.push((rt.range(), r.to_string()));
                    }
                    if c != col {
                        edits.push((ct.range(), c.to_string()));
                    }
                }
            }
            // Objects a save dropped and an undo brought back.
            let missing: String = edited
                .iter()
                .filter(|e| !seen.contains(&e.id))
                .filter_map(|e| {
                    let a = e.anchor?;
                    let wrapped = format!("<r {}>{}</r>", e.namespaces, e.xml);
                    let doc = Document::parse(&wrapped).ok()?;
                    let el = doc.root_element().first_element_child()?;
                    Some(placed_anchor(el, &a, &wrapped))
                })
                .collect();
            if !missing.is_empty() {
                let end = root.range().end;
                let close = xml[..end].rfind("</").ok_or("drawing root not closed")?;
                edits.push((close..close, missing));
            }
        }
        "pivot" => {
            // Where the pivot table is (its page fields are above it).
            if let Some(a) = doc
                .root_element()
                .children()
                .find(|c| c.is_element() && c.tag_name().name() == "location")
                .and_then(|l| l.attributes().find(|a| a.name() == "ref"))
            {
                match moves.sqref(home, a.value()) {
                    Some(v) if v != a.value() => edits.push((a.range_value(), esc_attr(&v))),
                    Some(_) => {}
                    None => return Err("its cells were deleted".into()),
                }
            }
        }
        "pivotCache" => {
            // A source range on a sheet (a source by name follows the name).
            for src in doc
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "worksheetSource")
            {
                let (Some(r), Some(sheet)) = (
                    src.attributes().find(|a| a.name() == "ref"),
                    src.attribute("sheet"),
                ) else {
                    continue;
                };
                let Some(&id) = m.sheets.get(&sheet.to_lowercase()) else {
                    continue;
                };
                match moves.sqref(id, r.value()) {
                    Some(v) if v != r.value() => edits.push((r.range_value(), esc_attr(&v))),
                    Some(_) => {}
                    None => return Err("its source cells were deleted".into()),
                }
            }
        }
        _ => {
            // Charts: series and category references.
            for f in doc
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "f")
            {
                let Some(t) = text_of(f) else { continue };
                let text = t.text().unwrap_or_default();
                let v = moves.formula(text, None, &m.sheets);
                if v != text {
                    edits.push((t.range(), esc_text(&v)));
                }
            }
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    Ok(Some(apply_edits(xml, edits).into_bytes()))
}

/// An anchor element (`src[a.range()]`) with its position elements
/// replaced by `anchor`'s, as the same kind of anchor.
fn placed_anchor(a: Node, anchor: &crate::drawing::Anchor, src: &str) -> String {
    let text = &src[a.range()];
    let tag = text[1..]
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or_default();
    let prefix = tag.strip_suffix(a.tag_name().name()).unwrap_or_default();
    // The open tag as written (unless the kind of anchor changed), and the
    // children that aren't its position.
    let open_end = a
        .first_child()
        .map_or(text.len(), |c| c.range().start - a.range().start);
    let kind = match anchor {
        crate::drawing::Anchor::TwoCell { .. } => "twoCellAnchor",
        crate::drawing::Anchor::OneCell { .. } => "oneCellAnchor",
        crate::drawing::Anchor::Absolute { .. } => "absoluteAnchor",
    };
    let same = kind == a.tag_name().name();
    let tag = if same {
        tag.to_owned()
    } else {
        format!("{prefix}{kind}")
    };
    let mut out = if same {
        text[..open_end].to_owned()
    } else {
        format!("<{tag}>")
    };
    out.push_str(&anchor.position_xml(prefix));
    for c in a.children() {
        if c.is_element() && matches!(c.tag_name().name(), "from" | "to" | "ext" | "pos") {
            continue;
        }
        out.push_str(&src[c.range()]);
    }
    out.push_str(&format!("</{tag}>"));
    out
}

/// The parts of a package by name (for tests).
#[cfg(test)]
pub(crate) fn parts_of(bytes: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
    let pkg = Package::read(bytes).expect("zip");
    pkg.entries
        .into_iter()
        .filter(|(n, _)| !n.ends_with('/'))
        .map(|(n, d)| (n, d.as_ref().clone()))
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
        let ctx = Context {
            tabs: &tabs,
            full_calc: false,
            moves: &Moves::default(),
            cell_text: &|_, _, _| String::new(),
            tables: &|_| None,
            drawings: &HashMap::new(),
            pivots: &PivotSave::default(),
            patches: &Patches::default(),
        };
        let done = finish(out, original, &ctx);
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

    fn corpus_file(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/corpus/xlsx")
                .join(name),
        )
        .unwrap()
    }

    /// `(name, sheetId)` of the `<sheet>`s of a saved workbook part.
    fn sheet_list(parts: &std::collections::BTreeMap<String, Vec<u8>>) -> Vec<(String, u32)> {
        let wb = text(parts, "xl/workbook.xml");
        let doc = Document::parse(&wb).unwrap();
        doc.descendants()
            .filter(|n| n.has_tag_name((NS_MAIN, "sheet")))
            .map(|n| {
                (
                    n.attribute("name").unwrap().to_owned(),
                    n.attribute("sheetId").unwrap().parse().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn chart_sheets_keep_their_place_and_a_free_id() {
        let original = corpus_file("14_charts_images.xlsx");
        let m = model(&original);
        let parts = parts_of(&save(&m, Some(&original)).bytes);
        assert_eq!(
            sheet_list(&parts),
            [("Data".into(), 1), ("Trend".into(), 2), ("After".into(), 3)]
        );
        // "After" deleted and a sheet added: IronCalc gives it id 2, the
        // chart sheet's.
        let mut m = model(&original);
        m.delete_sheet(1).unwrap();
        m.new_sheet();
        let done = save(&m, Some(&original));
        let parts = parts_of(&done.bytes);
        let list = sheet_list(&parts);
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[1].0, "Trend");
        let mut ids: Vec<u32> = list.iter().map(|s| s.1).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "sheet ids are unique: {list:?}");
        assert!(parts.contains_key("xl/chartsheets/sheet1.xml"));
        load_from_xlsx_bytes(&done.bytes, "t", "en", "UTC").unwrap();
    }

    #[test]
    fn skeletons_leave_out_the_cells() {
        let body: String = (1..=60000)
            .map(|r| format!("<row r=\"{r}\"><c r=\"A{r}\"><v>{r}</v></c></row>"))
            .collect();
        let xml = format!(
            "<worksheet xmlns=\"{NS_MAIN}\"><sheetPr/><sheetData>{body}</sheetData><pageMargins left=\"1\"/></worksheet>"
        );
        let empty = format!("<worksheet xmlns=\"{NS_MAIN}\"><sheetData/><drawing/></worksheet>");
        let mut pkg = Package::default();
        pkg.set("big.xml", xml.clone().into_bytes());
        pkg.set("empty.xml", empty.clone().into_bytes());
        let bytes = pkg.write().unwrap();
        let orig = Original::new(&bytes).unwrap();
        for (name, full) in [("big.xml", &xml), ("empty.xml", &empty)] {
            let streamed = orig.skeleton(name).unwrap().unwrap();
            let direct = Skeleton::new(full);
            assert_eq!(streamed.xml, direct.xml, "{name}");
            assert_eq!(streamed.cut, direct.cut, "{name}");
        }
        let sk = Skeleton::new(&xml);
        assert_eq!(
            sk.xml,
            format!(
                "<worksheet xmlns=\"{NS_MAIN}\"><sheetPr/><sheetData></sheetData><pageMargins left=\"1\"/></worksheet>"
            )
        );
        // Offsets after the cells map back into the full XML.
        let at = sk.xml.find("<pageMargins").unwrap();
        assert_eq!(&xml[sk.full_offset(at)..][..12], "<pageMargins");
    }

    #[test]
    fn light_read_back_checks_all_but_the_cells() {
        let original = rich_fixture("");
        let pkg = Package::read(&original).unwrap();
        // Without the cells IronCalc still reads it, and the bytes are whole.
        let bytes = read_back(&pkg, 0).unwrap();
        assert_eq!(bytes.len(), pkg.write().unwrap().len());
        // Broken markup outside the cells is still caught.
        let mut broken = pkg.clone();
        let xml = broken
            .text("xl/workbook.xml")
            .unwrap()
            .unwrap()
            .replace("</sheets>", "");
        broken.set("xl/workbook.xml", xml.into_bytes());
        assert!(read_back(&broken, 0).is_err());
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
    fn patches_keep_unmodelled_attributes_and_order() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetPr codeName="S"/><dimension ref="A1"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageMargins left="1" right="1" top="1" bottom="1" header="0.5" footer="0.5"/><pageSetup paperSize="1" copies="2" r:id="rId4"/><drawing r:id="rId1"/></worksheet>"#;
        let patch = SheetPatch {
            elements: vec![
                ("printOptions", Some("<printOptions gridLines=\"1\"/>".into())),
                ("pageMargins", Some("<pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/>".into())),
                ("headerFooter", Some("<headerFooter><oddFooter>&amp;P</oddFooter></headerFooter>".into())),
            ],
            fit_to_page: Some(true),
            page_setup: Some(vec![("paperSize", "9".into()), ("orientation", "landscape".into())]),
            remove_exts: Vec::new(),
            frozen_view: None,
        };
        let out = patch_sheet(xml, &patch).unwrap();
        assert_eq!(
            out,
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetPr codeName="S"><pageSetUpPr fitToPage="1"/></sheetPr><dimension ref="A1"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><printOptions gridLines="1"/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" orientation="landscape" copies="2" r:id="rId4"/><headerFooter><oddFooter>&amp;P</oddFooter></headerFooter><drawing r:id="rId1"/></worksheet>"#
        );
        // Turning fit off leaves the other pageSetUpPr attributes.
        let off = SheetPatch {
            fit_to_page: Some(false),
            ..SheetPatch::default()
        };
        let out = patch_sheet(&out, &off).unwrap();
        assert!(
            out.contains("<sheetPr codeName=\"S\"><pageSetUpPr/></sheetPr>"),
            "{out}"
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
