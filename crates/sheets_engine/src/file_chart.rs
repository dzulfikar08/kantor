//! Restyling charts that came with the file in the Chart panel.
//!
//! The chart part is changed in place by `chart_style` (only the elements a
//! change owns are rewritten; extensions, unknown elements, manual layouts
//! and text formatting stay byte for byte). The edited part lives in the
//! history state's [`crate::edits::Edits`] with the layout it was made in,
//! its references in that layout's rows and columns; [`Workbook::drawings`]
//! shows it with its references moved to the current ones. A save writes
//! the current part of every chart edited this session over the carried
//! one (found through the sheet's drawing by `cNvPr id`), so an edit saved
//! and then undone is undone in the next file too.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::chart_edit::{ChartInfo, ChartSpec, ChartType, NEW_CHART_BASE};
use crate::chart_style;
use crate::drawing::{DrawingKey, chart_sheet_of_key};
use crate::shift::Layout;
use crate::{CellRange, Workbook};

/// An edited chart part to write at save.
pub(crate) struct FileChartSave {
    pub sheet_id: u32,
    /// The chart sheet part it is on (`None`: on worksheet `sheet_id`).
    pub chart_sheet: Option<String>,
    /// `cNvPr id` of its frame in the sheet's drawing.
    pub cnvpr: u32,
    pub xml: String,
}

/// A reference without `$` and quotes, upper case (for comparing).
fn plain(f: &str) -> String {
    f.replace(['$', '\''], "").to_uppercase()
}

impl Workbook {
    /// Lower-case sheet names -> `sheetId`, now.
    fn sheet_ids_now(&self) -> HashMap<String, u32> {
        self.sheet_names()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lowercase(), self.sheet_id(i as u32)))
            .collect()
    }

    /// A chart part with its references moved from `layout` to now.
    pub(crate) fn refs_now(&self, xml: &str, layout: &Layout) -> String {
        let moves = layout.moves_to(&self.history.layout());
        if moves.is_empty() {
            return xml.to_owned();
        }
        let names = self.sheet_ids_now();
        chart_style::map_formulas(xml, |f| moves.formula(f, None, &names).into_owned())
    }

    /// The part of file chart `key` as it is now (edited or as opened),
    /// with its references in the current rows and columns, and whether it
    /// was edited. `None`: not a chart of the file Kantor can save edits of.
    pub(crate) fn file_chart_now(&self, key: DrawingKey) -> Option<(String, bool)> {
        let (_, xml) = self.drawings.chart_source(key)?;
        let edits = self.edits();
        Some(match edits.file_charts.get(&key) {
            Some((x, layout)) => (self.refs_now(x, layout), true),
            None => (self.refs_now(&xml, &Layout::default()), false),
        })
    }

    /// Whether file chart `key` is there now (not deleted, nor its sheet).
    fn file_chart_shown(&self, key: DrawingKey) -> bool {
        if chart_sheet_of_key(key.sheet_id).is_some() {
            return self.chart_sheet_chart_shown(key);
        }
        self.sheet_index_of(key.sheet_id).is_some_and(|s| {
            self.drawings(s)
                .iter()
                .any(|d| d.key == key && d.editable)
        })
    }

    /// The sheet (an index) a chart's data defaults to: its own; a chart
    /// sheet's charts name theirs, else the first worksheet.
    fn chart_home(&self, key: DrawingKey) -> Option<u32> {
        match chart_sheet_of_key(key.sheet_id) {
            Some(_) => Some(0),
            None => self.sheet_index_of(key.sheet_id),
        }
    }

    /// See [`Workbook::chart_info`].
    pub(crate) fn file_chart_info(&self, key: DrawingKey) -> Option<ChartInfo> {
        // Deleted charts aren't shown.
        if !self.file_chart_shown(key) {
            return None;
        }
        let (xml, _) = self.file_chart_now(key)?;
        let style = chart_style::read(&xml).ok()?;
        // The data: the block every reference lies in, on one sheet.
        let mut sheet: Option<String> = None;
        let mut area: Option<CellRange> = None;
        let mut by_rows = false;
        let mut unknown = style.refs.is_empty();
        for (i, refs) in style.refs.iter().enumerate() {
            if refs[2].is_none() {
                unknown = true;
            }
            for (k, f) in refs.iter().enumerate() {
                let Some(f) = f else { continue };
                let tokens = crate::shift::references(f);
                if tokens.len() != 1 {
                    unknown = true;
                }
                for t in tokens {
                    let Some(s) = t.sheet.clone() else {
                        unknown = true;
                        continue;
                    };
                    match &sheet {
                        Some(s0) if !s0.eq_ignore_ascii_case(&s) => unknown = true,
                        Some(_) => {}
                        None => sheet = Some(s),
                    }
                    let a = t.range.area();
                    let r = CellRange::from_corners((a.top, a.left), (a.bottom, a.right));
                    if i == 0 && k == 2 {
                        by_rows = a.top == a.bottom && a.left != a.right;
                    }
                    area = Some(match area {
                        None => r,
                        Some(b) => CellRange::from_corners(
                            (b.top.min(r.top), b.left.min(r.left)),
                            (b.bottom.max(r.bottom), b.right.max(r.right)),
                        ),
                    });
                }
            }
        }
        let data_sheet = sheet.as_deref().and_then(|s| {
            let i = self
                .sheet_names()
                .iter()
                .position(|n| n.eq_ignore_ascii_case(s))?;
            Some(self.sheet_id(i as u32))
        });
        if data_sheet.is_none() {
            unknown = true;
        }
        let mut spec = ChartSpec {
            kind: style.kind,
            title: style.title.clone(),
            legend: style.legend,
            by_rows,
            data: area.unwrap_or_else(|| CellRange::from_corners((0, 0), (0, 0))),
            data_sheet: data_sheet.filter(|id| *id != key.sheet_id),
            x_title: style.x_title.clone(),
            y_title: style.y_title.clone(),
            colors: style.colors.clone(),
        };
        // Can the range give these series back? (Else a new range replaces
        // them, and the panel says so.)
        if !unknown && !style.fixed {
            let sheet_now = self.chart_home(key)?;
            let mut probe = spec.clone();
            if probe.kind == ChartType::Other {
                probe.kind = ChartType::Column;
            }
            let ds = self.data_sheet_index(sheet_now, &probe);
            let made = self.series_xml(ds, &probe);
            let got: Vec<Option<String>> = made
                .iter()
                .map(|s| chart_style::first_formula(&s.val).map(|f| plain(&f)))
                .collect();
            let want: Vec<Option<String>> = style
                .refs
                .iter()
                .map(|r| r[2].as_deref().map(plain))
                .collect();
            if got != want {
                unknown = true;
            }
        }
        if unknown {
            spec.by_rows = by_rows;
        }
        Some(ChartInfo {
            from_file: true,
            kind_label: style.kind_label,
            fixed: style.fixed,
            data_unknown: unknown,
            has_axes: style.has_axes,
            series: style.names,
            spec,
        })
    }

    /// See [`Workbook::set_chart`].
    pub(crate) fn set_file_chart(&mut self, key: DrawingKey, spec: ChartSpec) -> Result<()> {
        debug_assert!(key.index < NEW_CHART_BASE);
        let info = self
            .file_chart_info(key)
            .ok_or_else(|| anyhow!("Kantor can't change this chart"))?;
        let old = &info.spec;
        if *old == spec {
            return Ok(());
        }
        let (mut xml, _) = self
            .file_chart_now(key)
            .ok_or_else(|| anyhow!("Kantor can't change this chart"))?;
        let sheet = self.chart_home(key).ok_or_else(|| anyhow!("no sheet"))?;
        let data_changed = old.data != spec.data
            || old.by_rows != spec.by_rows
            || old.data_sheet != spec.data_sheet;
        if data_changed {
            if info.fixed {
                bail!(
                    "Kantor can't change the data of a {} chart",
                    info.kind_label
                );
            }
            self.check_chart_data(sheet, &spec)?;
            let mut probe = spec.clone();
            if probe.kind == ChartType::Other {
                probe.kind = ChartType::Column;
            }
            let ds = self.data_sheet_index(sheet, &probe);
            let series = self.series_xml(ds, &probe);
            if series.is_empty() {
                bail!("The data range makes no series");
            }
            xml = chart_style::set_series(&xml, &series)?;
        }
        if spec.kind != old.kind {
            if info.fixed || spec.kind == ChartType::Other {
                bail!(
                    "Kantor can't change the type of a {} chart",
                    info.kind_label
                );
            }
            xml = chart_style::set_kind(&xml, spec.kind)?;
        }
        if spec.title != old.title {
            xml = chart_style::set_title(&xml, &spec.title)?;
        }
        if spec.legend != old.legend {
            xml = chart_style::set_legend(&xml, spec.legend)?;
        }
        if spec.x_title != old.x_title {
            xml = chart_style::set_axis_title(&xml, true, &spec.x_title)?;
        }
        if spec.y_title != old.y_title {
            xml = chart_style::set_axis_title(&xml, false, &spec.y_title)?;
        }
        let n = chart_style::read(&xml)?.colors.len();
        for i in 0..n {
            let new = spec.colors.get(i).copied().unwrap_or_default();
            let was = old.colors.get(i).copied().unwrap_or_default();
            if new != was {
                xml = chart_style::set_color(&xml, i, new)?;
            }
        }
        self.own_edit(|e, layout| {
            e.file_charts.insert(key, (xml.into(), layout.clone()));
        });
        Ok(())
    }

    /// File charts edited this session, for a save: their current parts.
    pub(crate) fn file_chart_patches(&self, out: &mut crate::preserve::Patches) {
        let mut keys: Vec<&DrawingKey> = self.touched.file_charts.iter().collect();
        keys.sort_by_key(|k| (k.sheet_id, k.index));
        for key in keys {
            if !self.file_chart_shown(*key) {
                continue; // deleted
            }
            let chart_sheet = match chart_sheet_of_key(key.sheet_id) {
                Some(k) => match self.drawings.chart_sheet_parts(k) {
                    Some(p) => Some(p.part),
                    None => continue,
                },
                None => None,
            };
            let (Some((cnvpr, _)), Some((xml, _))) =
                (self.drawings.chart_source(*key), self.file_chart_now(*key))
            else {
                continue;
            };
            out.file_charts.push(FileChartSave {
                sheet_id: key.sheet_id,
                chart_sheet,
                cnvpr,
                xml,
            });
        }
    }
}
