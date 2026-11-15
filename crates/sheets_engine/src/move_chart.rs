//! Move Chart, as Excel's dialog has it: "New sheet" puts a chart on a
//! chart sheet of its own (before the sheet it was on), "Object in" puts it
//! on a worksheet (a chart sheet's chart leaves its chart sheet, which
//! goes). One undo step either way.
//!
//! The chart keeps its part as it is now (restyled, references in the
//! current rows and columns): on a new chart sheet it is that chart sheet's
//! restyled chart (`Edits::file_charts`), on a worksheet a chart whose part
//! Kantor carries ([`crate::chart_edit::NewChart::xml`]). New chart sheets
//! are added to the session's list (`Drawings::add_chart_sheet`) and shown
//! by the history states that list them; a save writes their parts as a
//! chart sheet put back from its parts (`preserve::ChartSheetPlan`).

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::Document;

use crate::Workbook;
use crate::chart_edit::{NEW_CHART_BASE, NewChart};
use crate::drawing::{
    Anchor, ChartSheetNow, DrawingKey, EditAs, Marker, Tab, chart_sheet_key_id, chart_sheet_of_key,
};
use crate::preserve::BundlePart;
use crate::xls_drawing::{CHART_SHEET_SIZE, XDR, frame, rels};

pub(crate) const CHARTSHEET_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet";
const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RELS_CT: &str = "application/vnd.openxmlformats-package.relationships+xml";

/// Where Move Chart puts a chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChartPlace {
    /// A new chart sheet with this name.
    NewSheet(String),
    /// An object on this worksheet (index).
    ObjectIn(u32),
}

/// Where a moved chart is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovedChart {
    /// Chart sheet `k` (see [`Workbook::chart_sheet_name`]).
    ChartSheet(usize),
    /// On worksheet `sheet` as `key`.
    Object { sheet: u32, key: DrawingKey },
}

/// The parts of a chart sheet made in Kantor.
pub(crate) struct NewChartSheet {
    pub name: String,
    pub part: String,
    pub sheet_xml: String,
    pub drawing_xml: String,
    pub chart_part: String,
    pub chart_xml: String,
    pub bundle: Vec<BundlePart>,
}

/// A tag for new part names: different in every session, so a file saved
/// earlier never has them already.
fn tag(k: usize) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!("{:x}", t ^ ((k as u64) << 48))
}

/// A chart sheet's parts holding chart `chart_xml`, with print settings as
/// Excel makes them (landscape, the chart fitted to the page) and, when
/// given, a tab colour (`<tabColor>`'s attributes).
pub(crate) fn chart_sheet_parts(
    name: &str,
    k: usize,
    chart_xml: String,
    tab: Option<&str>,
) -> NewChartSheet {
    let t = tag(k);
    let part = format!("xl/chartsheets/kantor{t}.xml");
    let drawing = format!("xl/drawings/kantorcs{t}.xml");
    let chart_part = format!("xl/charts/kantorcs{t}.xml");
    let sheet_pr = match tab {
        Some(attrs) => format!("<sheetPr><tabColor {attrs}/></sheetPr>"),
        None => "<sheetPr/>".to_owned(),
    };
    let sheet_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<chartsheet xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_R}\">{sheet_pr}<sheetViews><sheetView zoomScale=\"100\" workbookViewId=\"0\" zoomToFit=\"1\"/></sheetViews><pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/><pageSetup orientation=\"landscape\"/><drawing r:id=\"rId1\"/></chartsheet>"
    );
    let (cx, cy) = CHART_SHEET_SIZE;
    let drawing_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr {XDR}><xdr:absoluteAnchor><xdr:pos x=\"0\" y=\"0\"/><xdr:ext cx=\"{cx}\" cy=\"{cy}\"/>{}<xdr:clientData/></xdr:absoluteAnchor></xdr:wsDr>",
        frame(2, "Chart 1", "", "", "rId1")
    );
    let file = |p: &str| p.rsplit('/').next().unwrap_or_default().to_owned();
    let rels_of = |p: &str| {
        let (dir, f) = p.rsplit_once('/').unwrap_or(("", p));
        format!("{dir}/_rels/{f}.rels")
    };
    let bundle = vec![
        BundlePart {
            name: part.clone(),
            data: sheet_xml.clone().into_bytes(),
            content_type:
                "application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml".into(),
        },
        BundlePart {
            name: rels_of(&part),
            data: rels(&[(
                "rId1".into(),
                "drawing",
                format!("../drawings/{}", file(&drawing)),
            )])
            .into_bytes(),
            content_type: RELS_CT.into(),
        },
        BundlePart {
            name: drawing.clone(),
            data: drawing_xml.clone().into_bytes(),
            content_type: "application/vnd.openxmlformats-officedocument.drawing+xml".into(),
        },
        BundlePart {
            name: rels_of(&drawing),
            data: rels(&[(
                "rId1".into(),
                "chart",
                format!("../charts/{}", file(&chart_part)),
            )])
            .into_bytes(),
            content_type: RELS_CT.into(),
        },
        BundlePart {
            name: chart_part.clone(),
            data: chart_xml.clone().into_bytes(),
            content_type: "application/vnd.openxmlformats-officedocument.drawingml.chart+xml"
                .into(),
        },
    ];
    NewChartSheet {
        name: name.to_owned(),
        part,
        sheet_xml,
        drawing_xml,
        chart_part,
        chart_xml,
        bundle,
    }
}

/// `xml` without what links to other parts (`c:externalData`,
/// `c:userShapes`): a moved or copied chart part has no relationships.
pub(crate) fn without_links(xml: &str) -> String {
    let Ok(doc) = Document::parse(xml) else {
        return xml.to_owned();
    };
    let mut cut: Vec<std::ops::Range<usize>> = doc
        .descendants()
        .filter(|n| {
            n.is_element()
                && n.tag_name().namespace() == Some(crate::chart_style::NS_C)
                && matches!(n.tag_name().name(), "externalData" | "userShapes")
        })
        .map(|n| n.range())
        .collect();
    cut.sort_by_key(|r| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for r in cut {
        out.replace_range(r, "");
    }
    out
}

/// Insert Chart's place and size for a chart with no place yet: from B2,
/// 8 columns by 15 rows.
pub(crate) fn default_anchor() -> Anchor {
    Anchor::TwoCell {
        from: Marker {
            col: 1,
            col_off: 0,
            row: 1,
            row_off: 0,
        },
        to: Marker {
            col: 9,
            col_off: 0,
            row: 16,
            row_off: 0,
        },
        edit_as: EditAs::OneCell,
    }
}

/// Where a chart to be moved is now.
enum From {
    /// A file's chart on a worksheet.
    File,
    /// A chart made in Kantor (or moved or copied here).
    New(Box<NewChart>),
    /// The chart of chart sheet `k`.
    ChartSheet(usize),
}

impl Workbook {
    /// The name Move Chart suggests for a new sheet: `Chart` and the first
    /// number no sheet has.
    pub fn next_chart_sheet_name(&self) -> String {
        (1..)
            .map(|n| format!("Chart{n}"))
            .find(|n| self.sheet_name_taken(n, None).is_ok())
            .unwrap_or_default()
    }

    /// `sheetId`s of the worksheets before tab `tab` (all tabs, hidden
    /// ones too), nearest first.
    pub(crate) fn worksheets_before(&self, tab: Tab) -> Vec<u32> {
        let tabs = self.all_tabs();
        let end = tabs.iter().position(|t| *t == tab).unwrap_or(tabs.len());
        tabs[..end]
            .iter()
            .rev()
            .filter_map(|t| match t {
                Tab::Sheet(i) => Some(self.sheet_id(*i)),
                Tab::Chart(_) => None,
            })
            .collect()
    }

    /// Move chart `key` (one on a worksheet or a chart sheet's) to a new
    /// chart sheet or onto a worksheet. One undo step.
    pub fn move_chart(&mut self, key: DrawingKey, to: ChartPlace) -> Result<MovedChart> {
        let info = self
            .chart_info(key)
            .ok_or_else(|| anyhow!("Kantor can't move this chart"))?;
        let from = match chart_sheet_of_key(key.sheet_id) {
            Some(k) => From::ChartSheet(k),
            None if key.index >= NEW_CHART_BASE => From::New(Box::new(
                self.new_chart(key)
                    .ok_or_else(|| anyhow!("Kantor can't move this chart"))?,
            )),
            None => From::File,
        };
        let xml = match &from {
            From::New(c) => self.new_chart_part(c),
            _ => {
                self.file_chart_now(key)
                    .ok_or_else(|| anyhow!("Kantor can't move this chart"))?
                    .0
            }
        };
        let xml = without_links(&xml);
        match to {
            ChartPlace::NewSheet(name) => {
                let name = name.trim().to_owned();
                if let From::ChartSheet(k) = from {
                    // Already on a sheet of its own: Excel's dialog renames.
                    self.rename_chart_sheet(k, &name)?;
                    return Ok(MovedChart::ChartSheet(k));
                }
                self.sheet_name_taken(&name, None)?;
                let sheet = self
                    .sheet_index_of(key.sheet_id)
                    .ok_or_else(|| anyhow!("no sheet"))?;
                let after = self.worksheets_before(Tab::Sheet(sheet));
                let k = self.drawings.chart_sheet_count();
                let parts = chart_sheet_parts(&name, k, xml.clone(), None);
                let k = self.drawings.add_chart_sheet(parts);
                let mut list = self.chart_sheets_now();
                list.push(ChartSheetNow {
                    k,
                    name,
                    state: None,
                    after,
                    tab: None,
                });
                let cs_key = DrawingKey {
                    sheet_id: chart_sheet_key_id(k),
                    index: 0,
                };
                let add = move |e: &mut crate::edits::Edits, layout: &crate::shift::Layout| {
                    e.chart_sheets = Some(Arc::new(list));
                    e.file_charts.insert(cs_key, (xml.into(), layout.clone()));
                };
                match from {
                    From::File => {
                        self.delete_drawing(key)?;
                        self.change_edits(add);
                    }
                    From::New(c) => self.own_edit(|e, layout| {
                        e.charts.retain(|x| x.id != c.id);
                        add(e, layout);
                    }),
                    From::ChartSheet(_) => unreachable!(),
                }
                Ok(MovedChart::ChartSheet(k))
            }
            ChartPlace::ObjectIn(sheet) => {
                let target = self.sheet_id(sheet);
                if sheet as usize >= self.sheet_names().len() {
                    bail!("there's no such sheet");
                }
                if target == key.sheet_id {
                    return Ok(MovedChart::Object { sheet, key });
                }
                let anchor = match &from {
                    From::ChartSheet(_) => default_anchor(),
                    _ => self
                        .drawings(self.sheet_index_of(key.sheet_id).unwrap_or(0))
                        .iter()
                        .find(|d| d.key == key)
                        .map_or_else(default_anchor, |d| d.anchor),
                };
                let id = self
                    .edits()
                    .charts
                    .iter()
                    .map(|c| c.id)
                    .max()
                    .map_or(1, |m| m + 1);
                let spec = info.spec.clone();
                let layout = self.history.layout();
                let chart = NewChart {
                    id,
                    sheet_id: target,
                    spec,
                    anchor,
                    layout,
                    xml: Some(xml.into()),
                    picture: None,
                };
                match from {
                    From::File => {
                        self.delete_drawing(key)?;
                        self.change_edits(|e, _| e.charts.push(chart));
                    }
                    From::New(c) => self.own_edit(|e, _| {
                        e.charts.retain(|x| x.id != c.id);
                        e.charts.push(chart);
                    }),
                    From::ChartSheet(k) => {
                        let mut list = self.chart_sheets_now();
                        list.retain(|c| c.k != k);
                        self.own_edit(|e, _| {
                            e.chart_sheets = Some(Arc::new(list));
                            e.charts.push(chart);
                        });
                    }
                }
                Ok(MovedChart::Object {
                    sheet,
                    key: DrawingKey {
                        sheet_id: target,
                        index: NEW_CHART_BASE + id as usize,
                    },
                })
            }
        }
    }
}

/// Formula `f` with references to sheet `old` (`old!`, `'old'!`, any
/// case) made references to sheet `new`.
pub(crate) fn rename_sheet_in(f: &str, old: &str, new: &str) -> String {
    let quoted = format!("'{}'!", old.replace('\'', "''")).to_lowercase();
    let plain = format!("{old}!").to_lowercase();
    let to = format!("{}!", crate::print::quote_sheet(new));
    let lower = f.to_lowercase();
    let mut out = String::with_capacity(f.len());
    let mut i = 0;
    let mut in_quote = false;
    while i < f.len() {
        let boundary = i == 0
            || f[..i]
                .chars()
                .next_back()
                .is_some_and(|c| "(,;= +-*/&:".contains(c));
        if !in_quote && boundary {
            if lower[i..].starts_with(&quoted) {
                out.push_str(&to);
                i += quoted.len();
                continue;
            }
            if lower[i..].starts_with(&plain) {
                out.push_str(&to);
                i += plain.len();
                continue;
            }
        }
        let c = f[i..].chars().next().expect("in range");
        if c == '\'' {
            in_quote = !in_quote;
        }
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_references_renamed() {
        assert_eq!(
            rename_sheet_in("Data!$B$2:$B$5", "Data", "Data (2)"),
            "'Data (2)'!$B$2:$B$5"
        );
        assert_eq!(
            rename_sheet_in("('My data'!$A$1,MyData!$B$1)", "My data", "X"),
            "(X!$A$1,MyData!$B$1)"
        );
        assert_eq!(rename_sheet_in("Other!$A$1", "Data", "X"), "Other!$A$1");
        assert!(without_links("<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"r\"><c:chart/><c:externalData r:id=\"rId1\"/></c:chartSpace>").ends_with("<c:chart/></c:chartSpace>"));
    }
}
