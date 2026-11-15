//! Worksheet tabs, as Excel's tab menu has them: Insert, Delete, Rename,
//! Move or Copy, Tab Color, Hide and Unhide. Chart sheets' tabs share Move
//! (`move_tab`); their other commands are in `chart_sheet`.
//!
//! IronCalc does the worksheet itself (with the vendored patch §23: a
//! deleted sheet's references become `#REF!`, a copy takes a given name and
//! place). What Kantor keeps beside IronCalc follows in the same undo step:
//! a copy gets the source's notes, data validation and print settings; the
//! chart sheets keep their place among the worksheets (their `after` lists,
//! see [`crate::drawing::ChartSheetNow`]).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::types::{Color, SheetState};

use crate::Workbook;
use crate::chart_sheet::sheet_name_problem;
use crate::drawing::{ChartSheetNow, Tab};

/// A tab's colour, as Excel stores it: a theme colour with a tint, or a
/// colour of its own (`0xRRGGBB`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabColor {
    /// Theme slot (0 Background 1, 1 Text 1, 2 Background 2, 3 Text 2,
    /// 4-9 Accent 1-6) and tint (-1 darker … 1 lighter).
    Theme {
        slot: u8,
        tint: f64,
    },
    Rgb(u32),
}

/// The tints of the five rows under each theme colour in Excel's colour
/// menus (lighter rows first), by theme slot.
pub fn theme_tints(slot: u8) -> [f64; 5] {
    match slot {
        0 => [-0.05, -0.15, -0.25, -0.35, -0.5],
        1 => [0.5, 0.35, 0.25, 0.15, 0.05],
        2 => [-0.1, -0.25, -0.5, -0.75, -0.9],
        _ => [0.8, 0.6, 0.4, -0.25, -0.5],
    }
}

/// Excel's "Standard Colors" row.
pub const STANDARD_COLORS: [u32; 10] = [
    0xC00000, 0xFF0000, 0xFFC000, 0xFFFF00, 0x92D050, 0x00B050, 0x00B0F0, 0x0070C0, 0x002060,
    0x7030A0,
];

/// A worksheet or a chart sheet in a tab order being worked out.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    /// A worksheet by `sheetId`.
    Sheet(u32),
    Chart(usize),
}

fn hex(rgb: &str) -> Option<u32> {
    let h = rgb.strip_prefix('#').unwrap_or(rgb);
    let h = if h.len() == 8 { &h[2..] } else { h };
    u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6)
}

impl Workbook {
    /// The worksheet now called `name` (any case), or else the one called
    /// that when the file was opened (renamed since).
    pub(crate) fn sheet_index_by_name(&self, name: &str) -> Option<u32> {
        let names = self.sheet_names();
        if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            return Some(i as u32);
        }
        let id = *self.drawings.names.get(&name.to_lowercase())?;
        self.sheet_index_of(id)
    }

    /// Worksheets renamed since the file was opened: `sheetId` -> name now.
    pub(crate) fn renamed_since_open(&self) -> HashMap<u32, String> {
        let names = self.sheet_names();
        let mut out = HashMap::new();
        for (opened, &id) in &self.drawings.names {
            if let Some(i) = self.sheet_index_of(id) {
                let now = &names[i as usize];
                if now.to_lowercase() != *opened {
                    out.insert(id, now.clone());
                }
            }
        }
        out
    }

    /// Whether any worksheet is hidden (`hidden` only; Excel doesn't list
    /// "very hidden" ones either) as `(index, name)`, for Unhide.
    pub fn hidden_sheets(&self) -> Vec<(u32, String)> {
        self.model
            .get_model()
            .workbook
            .worksheets
            .iter()
            .enumerate()
            .filter(|(_, w)| w.state == SheetState::Hidden)
            .map(|(i, w)| (i as u32, w.name.clone()))
            .collect()
    }

    /// Worksheets shown (not hidden).
    fn visible_sheet_count(&self) -> usize {
        self.model
            .get_model()
            .workbook
            .worksheets
            .iter()
            .filter(|w| w.state == SheetState::Visible)
            .count()
    }

    fn check_sheet(&self, sheet: u32) -> Result<()> {
        if sheet as usize >= self.sheet_names().len() {
            bail!("there's no such sheet");
        }
        Ok(())
    }

    /// Whether `name` can't be a new name of `sheet` (a worksheet index, or
    /// `None` for a new sheet): Excel's rules, and no other sheet (chart
    /// sheets too) may have it in any case.
    pub fn sheet_name_taken(&self, name: &str, sheet: Option<u32>) -> Result<()> {
        if let Some(problem) = sheet_name_problem(name) {
            bail!("{problem}");
        }
        let lower = name.to_lowercase();
        let taken = self
            .sheet_names()
            .iter()
            .enumerate()
            .any(|(i, n)| Some(i as u32) != sheet && n.to_lowercase() == lower)
            || self
                .chart_sheets_now()
                .iter()
                .any(|c| c.name.to_lowercase() == lower);
        if taken {
            bail!("That name is already taken. Try a different one.");
        }
        Ok(())
    }

    fn name_free(&self, name: &str) -> bool {
        self.sheet_name_taken(name, None).is_ok()
    }

    /// The name Insert gives a new sheet: `Sheet` and the number after the
    /// highest `SheetN` there is.
    pub fn next_sheet_name(&self) -> String {
        let mut n = self
            .sheet_names()
            .into_iter()
            .chain(self.chart_sheets_now().into_iter().map(|c| c.name))
            .filter_map(|s| {
                s.get(..5).filter(|p| p.eq_ignore_ascii_case("sheet"))?;
                s.get(5..)?.parse::<u32>().ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        while !self.name_free(&format!("Sheet{n}")) {
            n += 1;
        }
        format!("Sheet{n}")
    }

    /// The name Move or Copy gives a copy of `name`: `name (2)`, `(3)`…,
    /// shortened to fit 31 characters.
    fn copy_name(&self, name: &str) -> String {
        // A copy of a copy counts on from its number.
        let base = match name.rsplit_once(" (") {
            Some((b, n))
                if n.strip_suffix(')')
                    .is_some_and(|d| d.parse::<u32>().is_ok()) =>
            {
                b
            }
            _ => name,
        };
        (2..)
            .map(|n| {
                let suffix = format!(" ({n})");
                let keep = 31 - suffix.chars().count();
                format!("{}{suffix}", base.chars().take(keep).collect::<String>())
            })
            .find(|c| self.name_free(c))
            .unwrap_or_default()
    }

    /// Whether `sheet` has anything Excel asks before deleting: cells with
    /// content or a style, notes, charts or pictures.
    pub fn sheet_has_data(&self, sheet: u32) -> bool {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return false;
        };
        ws.sheet_data.cells().next().is_some()
            || !self.notes_now(sheet).is_empty()
            || !self.drawings(sheet).is_empty()
    }

    /// The tabs as slots (worksheets by id), every tab, hidden ones too.
    fn slots(&self) -> Vec<Slot> {
        self.all_tabs()
            .into_iter()
            .map(|t| match t {
                Tab::Sheet(i) => Slot::Sheet(self.sheet_id(i)),
                Tab::Chart(k) => Slot::Chart(k),
            })
            .collect()
    }

    /// Where tab `at` of [`Workbook::tabs`] (shown tabs; past the end: the
    /// end) is among all of `order`'s slots.
    fn slot_at(&self, order: &[Slot], at: usize) -> usize {
        let shown = self.tabs();
        let Some(tab) = shown.get(at) else {
            return order.len();
        };
        let slot = match *tab {
            Tab::Sheet(i) => Slot::Sheet(self.sheet_id(i)),
            Tab::Chart(k) => Slot::Chart(k),
        };
        order.iter().position(|s| *s == slot).unwrap_or(order.len())
    }

    /// The chart sheets placed as in `order` (`None`: the file has none, or
    /// they keep their places).
    fn chart_sheets_in(&self, order: &[Slot]) -> Option<Vec<ChartSheetNow>> {
        if self.drawings.chart_sheet_count() == 0 {
            return None;
        }
        let list = self.chart_sheets_now();
        let out = self.place_chart_sheets(order, &list);
        (out != list).then_some(out)
    }

    /// Chart sheets `list` placed as in `order`.
    fn place_chart_sheets(&self, order: &[Slot], list: &[ChartSheetNow]) -> Vec<ChartSheetNow> {
        let mut out = Vec::with_capacity(list.len());
        let mut before: Vec<u32> = Vec::new();
        for s in order {
            match *s {
                Slot::Sheet(id) => before.insert(0, id),
                Slot::Chart(k) => {
                    if let Some(mut cs) = list.iter().find(|c| c.k == k).cloned() {
                        cs.after = before.clone();
                        out.push(cs);
                    }
                }
            }
        }
        // Any left out of the tabs (a worksheet has their name) stay.
        out.extend(
            list.iter()
                .filter(|c| !out.iter().any(|o| o.k == c.k))
                .cloned()
                .collect::<Vec<_>>(),
        );
        out
    }

    /// Record an IronCalc step that changed the sheets, with the chart
    /// sheets placed as `order` has them.
    fn sheets_edited(&mut self, order: &[Slot]) {
        let charts = self.chart_sheets_in(order);
        self.history.edited();
        if let Some(list) = charts {
            self.change_edits(|e, _| e.chart_sheets = Some(Arc::new(list)));
        }
    }

    /// Insert a new empty worksheet (`SheetN`) so it is tab `at` of
    /// [`Workbook::tabs`] (past the end: last), as Excel's Insert puts it
    /// before the sheet it was asked on. One undo step. Returns its index.
    pub fn insert_sheet(&mut self, at: usize) -> Result<u32> {
        let mut order = self.slots();
        let pos = self.slot_at(&order, at);
        let index = order[..pos]
            .iter()
            .filter(|s| matches!(s, Slot::Sheet(_)))
            .count() as u32;
        let name = self.next_sheet_name();
        self.model
            .kantor_insert_sheet(&name, index)
            .map_err(|e| anyhow!("{e}"))?;
        order.insert(pos, Slot::Sheet(self.sheet_id(index)));
        self.sheets_edited(&order);
        Ok(index)
    }

    /// Delete worksheet `sheet` (one undo step). References to it become
    /// `#REF!`; its own names go. At least one worksheet must stay shown.
    pub fn delete_sheet(&mut self, sheet: u32) -> Result<()> {
        self.check_sheet(sheet)?;
        let visible = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet)
            .is_ok_and(|w| w.state == SheetState::Visible);
        if visible && self.visible_sheet_count() <= 1 {
            bail!("A workbook must contain at least one visible worksheet");
        }
        let mut order = self.slots();
        let id = self.sheet_id(sheet);
        order.retain(|s| *s != Slot::Sheet(id));
        self.model
            .kantor_delete_sheet(sheet)
            .map_err(|e| anyhow!("{e}"))?;
        self.sheets_edited(&order);
        Ok(())
    }

    /// Rename worksheet `sheet` (one undo step): Excel's name rules, unique
    /// among all sheets. Formulas and names that refer to it follow.
    pub fn rename_sheet(&mut self, sheet: u32, name: &str) -> Result<()> {
        self.check_sheet(sheet)?;
        let name = name.trim();
        if self.sheet_names()[sheet as usize] == name {
            return Ok(());
        }
        self.sheet_name_taken(name, Some(sheet))?;
        self.model
            .rename_sheet(sheet, name)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Move a tab (a worksheet or a chart sheet) so it is tab `to` of
    /// [`Workbook::tabs`], counted once it is taken out (past the end:
    /// last). Hidden sheets keep their place among the others. One undo
    /// step.
    pub fn move_tab(&mut self, tab: Tab, to: usize) -> Result<()> {
        let mut shown = self.tabs();
        let Some(from) = shown.iter().position(|t| *t == tab) else {
            bail!("only a shown sheet can be moved");
        };
        shown.remove(from);
        let to = to.min(shown.len());
        if to == from {
            return Ok(());
        }
        let slot_of = |t: Tab| match t {
            Tab::Sheet(i) => Slot::Sheet(self.sheet_id(i)),
            Tab::Chart(k) => Slot::Chart(k),
        };
        let me = slot_of(tab);
        let mut order = self.slots();
        order.retain(|s| *s != me);
        let at = match shown.get(to) {
            Some(next) => {
                let next = slot_of(*next);
                order.iter().position(|s| *s == next).unwrap_or(order.len())
            }
            None => order.len(),
        };
        order.insert(at, me);
        match tab {
            Tab::Sheet(i) => {
                let new = order[..at]
                    .iter()
                    .filter(|s| matches!(s, Slot::Sheet(_)))
                    .count() as u32;
                if new != i {
                    self.model.move_sheet(i, new).map_err(|e| anyhow!("{e}"))?;
                    self.sheets_edited(&order);
                    return Ok(());
                }
            }
            Tab::Chart(_) => {}
        }
        if let Some(list) = self.chart_sheets_in(&order) {
            self.own_edit(|e, _| e.chart_sheets = Some(Arc::new(list)));
        }
        Ok(())
    }

    /// Move or Copy… with "Create a copy": a copy of worksheet `sheet`
    /// (`Name (2)`) so it is tab `at` of [`Workbook::tabs`] (past the end:
    /// last). Cells, formats, merged cells, column widths, row heights,
    /// conditional formats, the sheet's own names, notes, data validation,
    /// print settings, tables (renamed as Excel does: `Table1` → `Table12`),
    /// charts (their references to the sheet now to the copy) and pictures
    /// are copied. One undo step. Returns the copy's index.
    pub fn copy_sheet(&mut self, sheet: u32, at: usize) -> Result<u32> {
        self.check_sheet(sheet)?;
        let mut order = self.slots();
        let pos = self.slot_at(&order, at);
        let index = order[..pos]
            .iter()
            .filter(|s| matches!(s, Slot::Sheet(_)))
            .count() as u32;
        let source = self.sheet_names()[sheet as usize].clone();
        let name = self.copy_name(&source);
        // What Kantor keeps of the source, read before the sheets shift.
        let notes: Vec<crate::CellNote> = self.notes_now(sheet).values().cloned().collect();
        let validations = self.sheet_validations(sheet);
        let print = self.print_settings(sheet);
        let print_set = print != crate::SheetPrint::default();
        let objects = self.copied_objects(sheet, &source, &name);
        let mut tables: Vec<ironcalc_base::types::Table> = self
            .model
            .get_model()
            .workbook
            .tables
            .values()
            .filter(|t| t.sheet_name.eq_ignore_ascii_case(&source))
            .cloned()
            .collect();
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        self.batch(|wb| {
            wb.model
                .kantor_copy_sheet(sheet, &name, index)
                .map_err(|e| anyhow!("{e}"))?;
            let id = wb.sheet_id(index);
            order.insert(pos, Slot::Sheet(id));
            wb.sheets_edited(&order);
            for mut t in tables {
                let new = wb.copy_table_name(&t.name);
                t.name = new.clone();
                t.display_name = new.clone();
                t.sheet_name = name.clone();
                wb.model
                    .kantor_set_table(&new, Some(t))
                    .map_err(|e| anyhow!("{e}"))?;
                wb.history.edited();
                wb.touch_table(&new, None);
            }
            wb.change_edits(|e, layout| {
                if !notes.is_empty() {
                    e.notes.insert(id, (notes, layout.clone()));
                }
                if !validations.is_empty() {
                    e.validations.insert(id, (validations, layout.clone()));
                }
                if print_set {
                    e.prints.insert(id, (print, layout.clone()));
                }
                let mut next = e.charts.iter().map(|c| c.id).max().unwrap_or(0);
                for (anchor, xml, picture, spec) in objects {
                    next += 1;
                    e.charts.push(crate::chart_edit::NewChart {
                        id: next,
                        sheet_id: id,
                        spec,
                        anchor,
                        layout: layout.clone(),
                        xml,
                        picture,
                    });
                }
            });
            Ok(index)
        })
    }

    /// The name Excel gives a copy of table `name` on a copied sheet: the
    /// name with the first number from 2 that no table or name has.
    fn copy_table_name(&self, name: &str) -> String {
        let model = self.model.get_model();
        let taken = |n: &str| {
            model
                .workbook
                .tables
                .keys()
                .any(|t| t.eq_ignore_ascii_case(n))
                || model
                    .workbook
                    .defined_names
                    .iter()
                    .any(|d| d.name.eq_ignore_ascii_case(n))
        };
        (2..)
            .map(|k| format!("{name}{k}"))
            .find(|n| !taken(n))
            .unwrap_or_default()
    }

    /// The charts and pictures of `sheet` as objects of a copy named
    /// `copy`: anchor, chart part (references to `source` now to the copy)
    /// or picture, and the chart's spec. Hidden objects and shapes Kantor
    /// doesn't draw aren't copied.
    #[allow(clippy::type_complexity)]
    fn copied_objects(
        &self,
        sheet: u32,
        source: &str,
        copy: &str,
    ) -> Vec<(
        crate::drawing::Anchor,
        Option<Arc<str>>,
        Option<Arc<crate::chart_edit::NewPicture>>,
        crate::ChartSpec,
    )> {
        use crate::drawing::DrawingKind;
        let mut out = Vec::new();
        for d in self.drawings(sheet) {
            if d.hidden {
                continue;
            }
            match &*d.kind {
                DrawingKind::Chart {
                    xml, current_refs, ..
                } => {
                    let now = if *current_refs {
                        xml.to_string()
                    } else {
                        self.refs_now(xml, &crate::shift::Layout::default())
                    };
                    let now = crate::chart_style::map_formulas(&now, |f| {
                        crate::move_chart::rename_sheet_in(f, source, copy)
                    });
                    let spec = self
                        .chart_info(d.key)
                        .map_or_else(crate::ChartSpec::placeholder, |i| i.spec);
                    out.push((
                        d.anchor,
                        Some(crate::move_chart::without_links(&now).into()),
                        None,
                        spec,
                    ));
                }
                DrawingKind::Picture { part, data, crop } => {
                    let ext = part
                        .rsplit_once('.')
                        .map(|(_, e)| e.to_owned())
                        .filter(|e| e.len() <= 4 && !e.contains('/'))
                        .unwrap_or_else(|| "png".into());
                    out.push((
                        d.anchor,
                        None,
                        Some(Arc::new(crate::chart_edit::NewPicture {
                            data: data.clone(),
                            crop: *crop,
                            ext,
                            name: d.name.clone(),
                            descr: d.descr.clone(),
                        })),
                        crate::ChartSpec::placeholder(),
                    ));
                }
                DrawingKind::Other(_) => {}
            }
        }
        out
    }

    /// Move or Copy… with "Create a copy" on chart sheet `k`: a copy of it
    /// (`Name (2)`, with its chart and tab colour) so it is tab `at` of
    /// [`Workbook::tabs`]. One undo step. Returns the copy's `k`.
    pub fn copy_chart_sheet(&mut self, k: usize, at: usize) -> Result<usize> {
        let list = self.chart_sheets_now();
        let i = list
            .iter()
            .position(|c| c.k == k)
            .ok_or_else(|| anyhow!("there's no such chart sheet"))?;
        let key = self
            .chart_sheet_drawings(k)
            .into_iter()
            .find(|d| d.editable && matches!(*d.kind, crate::drawing::DrawingKind::Chart { .. }))
            .map(|d| d.key)
            .ok_or_else(|| anyhow!("Kantor can't copy this chart sheet"))?;
        let (xml, _) = self
            .file_chart_now(key)
            .ok_or_else(|| anyhow!("Kantor can't copy this chart sheet"))?;
        let xml = crate::move_chart::without_links(&xml);
        let name = self.copy_name(&list[i].name);
        let tab = list[i].tab.clone();
        let mut order = self.slots();
        let pos = self.slot_at(&order, at);
        let parts = crate::move_chart::chart_sheet_parts(
            &name,
            self.drawings.chart_sheet_count(),
            xml.clone(),
            tab.as_deref(),
        );
        let new = self.drawings.add_chart_sheet(parts);
        order.insert(pos, Slot::Chart(new));
        let mut with = list.clone();
        with.push(ChartSheetNow {
            k: new,
            name,
            state: None,
            after: Vec::new(),
            tab,
        });
        let placed = self.place_chart_sheets(&order, &with);
        let cs_key = crate::drawing::DrawingKey {
            sheet_id: crate::drawing::chart_sheet_key_id(new),
            index: 0,
        };
        self.own_edit(|e, layout| {
            e.chart_sheets = Some(Arc::new(placed));
            e.file_charts.insert(cs_key, (xml.into(), layout.clone()));
        });
        Ok(new)
    }

    /// Hide or unhide worksheet `sheet` (one undo step). At least one
    /// worksheet stays shown.
    pub fn set_sheet_hidden(&mut self, sheet: u32, hidden: bool) -> Result<()> {
        self.check_sheet(sheet)?;
        if hidden == self.sheet_hidden(sheet) {
            return Ok(());
        }
        if hidden && self.visible_sheet_count() <= 1 {
            bail!("A workbook must contain at least one visible worksheet");
        }
        let res = if hidden {
            self.model.hide_sheet(sheet)
        } else {
            self.model.unhide_sheet(sheet)
        };
        res.map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Worksheet `sheet`'s tab colour (`None`: no colour).
    pub fn tab_color(&self, sheet: u32) -> Option<TabColor> {
        let ws = self.model.get_model().workbook.worksheet(sheet).ok()?;
        match &ws.color {
            Color::Rgb(h) => hex(h).map(TabColor::Rgb),
            Color::Theme(slot, tint) => Some(TabColor::Theme {
                slot: u8::try_from(*slot).ok()?,
                tint: *tint,
            }),
            Color::None => None,
        }
    }

    /// The colour `c` is in this workbook's theme, `0xRRGGBB`.
    pub fn tab_color_rgb(&self, c: TabColor) -> u32 {
        match c {
            TabColor::Rgb(rgb) => rgb,
            TabColor::Theme { slot, tint } => {
                let theme = self.model.get_model().get_theme();
                hex(&theme.resolve(i32::from(slot), tint)).unwrap_or(0)
            }
        }
    }

    /// Tab Color: worksheet `sheet`'s tab gets `color` (`None`: No Color).
    /// One undo step.
    pub fn set_tab_color(&mut self, sheet: u32, color: Option<TabColor>) -> Result<()> {
        self.check_sheet(sheet)?;
        if self.tab_color(sheet) == color {
            return Ok(());
        }
        let c = match color {
            None => Color::None,
            Some(TabColor::Rgb(rgb)) => Color::Rgb(format!("#{:06X}", rgb & 0xFF_FFFF)),
            Some(TabColor::Theme { slot, tint }) => Color::Theme(i32::from(slot), tint),
        };
        self.model
            .set_sheet_color(sheet, &c)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }
}
