//! Sheet features edited in Kantor that IronCalc doesn't model (page setup,
//! ...): what each history state has, so they undo and redo with the cells.
//!
//! Each history state keeps an [`Edits`] (shared until it changes, like the
//! drawing edits and pivot states of `Revisions`). An edit makes a new state
//! with no IronCalc step ([`Workbook::own_edit`]). A save writes them into the
//! package (see `preserve::Patches`); sheets nobody edited keep what the file
//! had, byte for byte.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::Workbook;
use crate::print::SheetPrint;
use crate::shift::Layout;

/// What the current state changed, by `sheetId`.
#[derive(Clone, Default)]
pub(crate) struct Edits {
    /// Print settings set in Kantor, with the layout (rows and columns
    /// moved) they were set in: their ranges follow later moves.
    pub prints: HashMap<u32, (SheetPrint, Layout)>,
    /// Tables made or changed in Kantor, by lower-case name, with the names
    /// they had before (renamed): a save writes their parts from the model.
    pub tables: HashMap<String, Vec<String>>,
    /// Tables removed in Kantor (Convert to Range), lower case: a save
    /// doesn't carry their parts.
    pub removed_tables: HashSet<String>,
    /// Sheets (`sheetId`) whose conditional formatting rules were changed
    /// in Kantor: saved from the model.
    pub cf_sheets: HashSet<u32>,
    /// Data validation edited in Kantor: a sheet's whole list, with the
    /// layout it was made in.
    pub validations: HashMap<u32, (Vec<crate::Validation>, Layout)>,
    /// Charts made in Kantor (Insert Chart).
    pub charts: Vec<crate::chart_edit::NewChart>,
    /// Charts of the file restyled in Kantor: the edited part, its
    /// references in the rows and columns of the layout it was made in.
    pub file_charts: HashMap<crate::drawing::DrawingKey, (Arc<str>, Layout)>,
    /// Notes added, edited or deleted in Kantor: a sheet's whole list, at
    /// the cells of the layout it was made in.
    pub notes: HashMap<u32, (Vec<crate::CellNote>, Layout)>,
    /// The file's chart sheets once one was renamed, moved, hidden or
    /// deleted in Kantor (`None`: as opened).
    pub chart_sheets: Option<Arc<Vec<crate::drawing::ChartSheetNow>>>,
    /// Shapes and text boxes of the file whose text was edited in Kantor:
    /// their element's XML now (see [`crate::drawing::DrawingKind::Shape`]).
    pub shape_texts: HashMap<crate::drawing::DrawingKey, Arc<str>>,
    /// The workbook's custom table styles once one was made, changed or
    /// deleted in Kantor (`None`: as opened).
    pub table_styles: Option<Arc<crate::table_style_edit::StyleSet>>,
    /// Timelines' level and first period shown, set in Kantor, by
    /// `(sheetId, lower-case name)`.
    pub timelines: HashMap<(u32, String), (u32, Option<f64>)>,
    /// Timelines' settings changed in Kantor (Timeline Settings, styles,
    /// what shows), by `(sheetId, lower-case name)`.
    pub timeline_defs: HashMap<(u32, String), crate::slicer::TimelineDef>,
    /// Slicers' settings changed in Kantor (Slicer Settings, styles,
    /// columns), by `(sheetId, lower-case name)`.
    pub slicer_defs: HashMap<(u32, String), crate::slicer::SlicerDef>,
    /// Slicer caches' settings changed in Kantor (sorting, items with no
    /// data), by lower-case cache name.
    pub slicer_caches: HashMap<String, crate::slicer::CacheSettings>,
}

/// What was edited in Kantor at any point of the session (any history
/// state): a save writes these from the current state even when the current
/// state has no edit of its own (an edit saved, then undone, must be undone
/// in the next file too, whose original is the saved one).
#[derive(Default)]
pub(crate) struct Touched {
    pub prints: HashSet<u32>,
    pub cf: HashSet<u32>,
    pub validations: HashSet<u32>,
    pub tables: HashSet<String>,
    pub file_charts: HashSet<crate::drawing::DrawingKey>,
    pub notes: HashSet<u32>,
    pub shape_texts: HashSet<crate::drawing::DrawingKey>,
}

impl Workbook {
    /// The current state's edits.
    pub(crate) fn edits(&self) -> Arc<Edits> {
        self.history.edits()
    }

    /// Make a new history state (one undo step, no IronCalc step) whose
    /// edits are the current ones changed by `f`.
    pub(crate) fn own_edit(&mut self, f: impl FnOnce(&mut Edits, &Layout)) {
        self.history.edited_steps(0);
        self.change_edits(f);
    }

    /// Change the current state's edits (`f`) without a new state: for an
    /// edit that made one already (an IronCalc step) and changes features
    /// IronCalc doesn't model too.
    pub(crate) fn change_edits(&mut self, f: impl FnOnce(&mut Edits, &Layout)) {
        let layout = self.history.layout();
        let mut edits = (*self.history.edits()).clone();
        f(&mut edits, &layout);
        let t = &mut self.touched;
        t.prints.extend(edits.prints.keys().copied());
        t.cf.extend(edits.cf_sheets.iter().copied());
        t.validations.extend(edits.validations.keys().copied());
        t.tables.extend(edits.tables.keys().cloned());
        t.tables.extend(edits.tables.values().flatten().cloned());
        t.tables.extend(edits.removed_tables.iter().cloned());
        t.file_charts.extend(edits.file_charts.keys().copied());
        t.notes.extend(edits.notes.keys().copied());
        t.shape_texts.extend(edits.shape_texts.keys().copied());
        self.history.set_edits(Arc::new(edits));
        // Answers cached for this state since its IronCalc step are stale.
        *self.cache.borrow_mut() = crate::Cache::default();
    }
}
