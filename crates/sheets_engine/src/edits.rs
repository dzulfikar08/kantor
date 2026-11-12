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
}

impl Workbook {
    /// The current state's edits.
    pub(crate) fn edits(&self) -> Arc<Edits> {
        self.history.edits()
    }

    /// Make a new history state (one undo step, no IronCalc step) whose
    /// edits are the current ones changed by `f`.
    pub(crate) fn own_edit(&mut self, f: impl FnOnce(&mut Edits, &Layout)) {
        let layout = self.history.layout();
        let mut edits = (*self.history.edits()).clone();
        f(&mut edits, &layout);
        self.history.edited_steps(0);
        self.history.set_edits(Arc::new(edits));
    }
}
