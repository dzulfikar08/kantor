//! Chart sheets' tabs: Rename, Delete, Move, Hide and Unhide, as Excel's
//! tab menu has them.
//!
//! IronCalc has no chart sheets: the file's chart sheets are read at open
//! (`drawing::Drawings`) and their parts carried on save (`preserve`). A
//! history state that changed them keeps the whole list
//! ([`crate::edits::Edits::chart_sheets`]: names, states, places; deleted
//! ones left out), so each change is one undo step with no IronCalc step.
//! Every save writes the list of the current state ([`ChartSheetPlan`]): a
//! chart sheet is found in the package being carried from by its part name
//! (carrying keeps names), or, when an earlier save of the session left it
//! out (deleted, saved, then the deletion undone), put back from its parts
//! as opened.

use std::sync::Arc;

use anyhow::{Result, bail};

use crate::Workbook;
use crate::drawing::{ChartSheetNow, DrawingKey, Tab};
use crate::preserve::{BundlePart, ChartSheetPlan};
use crate::shift::Layout;

/// Why `name` can't be a sheet name (Excel's rules), if it can't.
pub fn sheet_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("A sheet name can't be blank");
    }
    if name.chars().count() > 31 {
        return Some("A sheet name can have at most 31 characters");
    }
    if name.contains(['\\', '/', '?', '*', '[', ']', ':']) {
        return Some("A sheet name can't contain \\ / ? * [ ] or :");
    }
    if name.starts_with('\'') || name.ends_with('\'') {
        return Some("A sheet name can't begin or end with an apostrophe");
    }
    if name.eq_ignore_ascii_case("History") {
        return Some("\u{201c}History\u{201d} is a name Excel keeps for itself");
    }
    None
}

impl Workbook {
    /// Make a history state with the chart sheets of `list`.
    fn set_chart_sheets(&mut self, list: Vec<ChartSheetNow>) {
        self.own_edit(|e, _| e.chart_sheets = Some(Arc::new(list)));
    }

    fn chart_sheet_index(list: &[ChartSheetNow], k: usize) -> Result<usize> {
        match list.iter().position(|c| c.k == k) {
            Some(i) => Ok(i),
            None => bail!("there's no such chart sheet"),
        }
    }

    /// Rename chart sheet `k` (one undo step). The name follows Excel's
    /// rules and no other sheet may have it (in any case).
    pub fn rename_chart_sheet(&mut self, k: usize, name: &str) -> Result<()> {
        let name = name.trim();
        if let Some(problem) = sheet_name_problem(name) {
            bail!("{problem}");
        }
        let mut list = self.chart_sheets_now();
        let i = Self::chart_sheet_index(&list, k)?;
        if list[i].name == name {
            return Ok(());
        }
        let lower = name.to_lowercase();
        let taken = self
            .sheet_names()
            .iter()
            .map(|n| n.to_lowercase())
            .chain(
                list.iter()
                    .filter(|c| c.k != k)
                    .map(|c| c.name.to_lowercase()),
            )
            .any(|n| n == lower);
        if taken {
            bail!("A sheet is already called \u{201c}{name}\u{201d}");
        }
        list[i].name = name.to_owned();
        self.set_chart_sheets(list);
        Ok(())
    }

    /// Delete chart sheet `k` (one undo step).
    pub fn delete_chart_sheet(&mut self, k: usize) -> Result<()> {
        let mut list = self.chart_sheets_now();
        let i = Self::chart_sheet_index(&list, k)?;
        list.remove(i);
        self.set_chart_sheets(list);
        Ok(())
    }

    /// Hide or unhide chart sheet `k` (one undo step). Worksheets always
    /// keep a tab, so hiding never leaves none.
    pub fn set_chart_sheet_hidden(&mut self, k: usize, hidden: bool) -> Result<()> {
        let mut list = self.chart_sheets_now();
        let i = Self::chart_sheet_index(&list, k)?;
        let state = hidden.then(|| "hidden".to_owned());
        if list[i].state == state {
            return Ok(());
        }
        list[i].state = state;
        self.set_chart_sheets(list);
        Ok(())
    }

    /// Move chart sheet `k`'s tab so it is tab `to` of [`Workbook::tabs`]
    /// (counted once it is taken out; past the end: last). One undo step.
    pub fn move_chart_sheet(&mut self, k: usize, to: usize) -> Result<()> {
        let list = self.chart_sheets_now();
        Self::chart_sheet_index(&list, k)?;
        let mut shown = self.tabs();
        let Some(from) = shown.iter().position(|t| *t == Tab::Chart(k)) else {
            bail!("only a shown chart sheet can be moved");
        };
        shown.remove(from);
        let to = to.min(shown.len());
        if to == from {
            return Ok(());
        }
        // Hidden chart sheets keep their place among all the tabs.
        let mut all = self.all_tabs();
        all.retain(|t| *t != Tab::Chart(k));
        let at = match shown.get(to) {
            Some(next) => all.iter().position(|t| t == next).unwrap_or(all.len()),
            None => all.len(),
        };
        all.insert(at, Tab::Chart(k));
        // Each chart sheet goes after the worksheets before it, nearest
        // first; chart sheets keep the order of the tabs.
        let mut out = Vec::with_capacity(list.len());
        let mut before: Vec<u32> = Vec::new();
        for t in &all {
            match *t {
                Tab::Sheet(i) => before.insert(0, self.sheet_id(i)),
                Tab::Chart(c) => {
                    if let Some(mut cs) = list.iter().find(|x| x.k == c).cloned() {
                        cs.after = before.clone();
                        out.push(cs);
                    }
                }
            }
        }
        // Any left out of the tabs (a worksheet has their name) stay.
        let rest: Vec<ChartSheetNow> = list
            .iter()
            .filter(|c| !out.iter().any(|o| o.k == c.k))
            .cloned()
            .collect();
        out.extend(rest);
        self.set_chart_sheets(out);
        Ok(())
    }

    /// Whether `key` is a chart on a chart sheet still there.
    pub(crate) fn chart_sheet_chart_shown(&self, key: DrawingKey) -> bool {
        crate::drawing::chart_sheet_of_key(key.sheet_id).is_some_and(|k| {
            self.chart_sheet_drawings(k)
                .iter()
                .any(|d| d.key == key && d.editable)
        })
    }

    /// What a save writes for the file's chart sheets (`None`: the file had
    /// none; the package's are carried as they are).
    pub(crate) fn chart_sheet_plans(&self) -> Option<Vec<ChartSheetPlan>> {
        if self.drawings.chart_sheet_count() == 0 {
            return None;
        }
        Some(
            self.chart_sheets_now()
                .into_iter()
                .filter_map(|c| {
                    let o = self.drawings.chart_sheet_parts(c.k)?;
                    Some(ChartSheetPlan {
                        name: c.name,
                        state: c.state,
                        after: c.after,
                        part: o.part,
                        rel_kind: o.rel_kind,
                        bundle: self.bundle_now(o.bundle, &o.charts),
                    })
                })
                .collect(),
        )
    }

    /// A chart sheet's parts as opened, its charts' references moved to the
    /// current rows and columns (the bundle is put back only when the
    /// package saved from no longer has the chart sheet; a restyled chart's
    /// part is written over it afterwards).
    fn bundle_now(
        &self,
        bundle: Arc<Vec<BundlePart>>,
        charts: &[(String, Arc<str>)],
    ) -> Arc<Vec<BundlePart>> {
        let moved: Vec<(&str, String)> = charts
            .iter()
            .map(|(part, xml)| (part.as_str(), self.refs_now(xml, &Layout::default())))
            .filter(|(_, now)| !bundle.iter().any(|b| b.data == now.as_bytes()))
            .collect();
        if moved.is_empty() {
            return bundle;
        }
        Arc::new(
            bundle
                .iter()
                .map(|b| match moved.iter().find(|(part, _)| *part == b.name) {
                    Some((_, now)) => BundlePart {
                        data: now.clone().into_bytes(),
                        ..b.clone()
                    },
                    None => b.clone(),
                })
                .collect(),
        )
    }
}
