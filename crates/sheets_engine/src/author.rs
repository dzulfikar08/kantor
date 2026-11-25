//! Edits for workbooks written from JSON (`kantor sheets edit|build` and
//! the MCP sheets tools) that the app's menus don't make: any style path
//! (number format codes, typefaces, sizes, indent, rotation), borders by
//! edge in any line style and colour, defined names, and charts whose part
//! is written elsewhere ([`crate::data_chart_part`] with cell references).
//!
//! Each method is one undo step.

use anyhow::{Result, anyhow, bail};

use crate::chart_edit::{ChartSpec, NEW_CHART_BASE, NewChart};
use crate::drawing::{Anchor, DrawingKey};
use crate::format::{LineStyle, Rgb, check_style_range};
use crate::{CellRange, Workbook};

/// Which edges of a range a border goes on (Excel's border menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderEdges {
    /// Every edge of every cell.
    All,
    /// The outline of the range.
    Outline,
    /// The lines between the cells, not the outline.
    Inside,
    Top,
    Bottom,
    Left,
    Right,
    /// The lines between rows (`CenterH`) / columns (`CenterV`).
    InsideHorizontal,
    InsideVertical,
    /// No borders.
    None,
}

impl BorderEdges {
    pub const NAMES: [&'static str; 10] = [
        "all",
        "outline",
        "inside",
        "top",
        "bottom",
        "left",
        "right",
        "inside_horizontal",
        "inside_vertical",
        "none",
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "all" => Self::All,
            "outline" => Self::Outline,
            "inside" => Self::Inside,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            "right" => Self::Right,
            "inside_horizontal" => Self::InsideHorizontal,
            "inside_vertical" => Self::InsideVertical,
            "none" => Self::None,
            _ => return None,
        })
    }

    fn ironcalc(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Outline => "Outer",
            Self::Inside => "Inner",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::InsideHorizontal => "CenterH",
            Self::InsideVertical => "CenterV",
            Self::None => "None",
        }
    }
}

/// A defined name: `scope` is the sheet (0-based) of a sheet-level name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinedName {
    pub name: String,
    pub scope: Option<u32>,
    /// What it refers to, without `=` (`Data!$A$1:$A$10`).
    pub formula: String,
}

impl Workbook {
    /// Set style `path` (IronCalc's: `num_fmt`, `font.name`, `font.size`,
    /// `font.b`, `alignment.indent`...) to `value` on every cell of `range`.
    pub fn set_range_style_value(
        &mut self,
        sheet: u32,
        range: CellRange,
        path: &str,
        value: &str,
    ) -> Result<()> {
        check_style_range(range)?;
        self.model
            .update_range_style(&range.area(sheet), path, value)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Draw `style` borders in `color` (`None`: automatic) on `edges` of
    /// `range` (`BorderEdges::None` removes them).
    pub fn set_range_border(
        &mut self,
        sheet: u32,
        range: CellRange,
        edges: BorderEdges,
        style: LineStyle,
        color: Option<Rgb>,
    ) -> Result<()> {
        check_style_range(range)?;
        let color = color.map_or_else(
            || "#000000".to_owned(),
            |c| format!("#{:06X}", c & 0xFF_FFFF),
        );
        let area: ironcalc_base::BorderArea = serde_json::from_value(serde_json::json!({
            "item": { "style": style.xml().to_ascii_lowercase(), "color": color },
            "type": edges.ironcalc(),
        }))?;
        self.model
            .set_area_with_border(&range.area(sheet), &area)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// The workbook's defined names (print areas and other built-in names
    /// included), as stored.
    pub fn defined_names(&self) -> Vec<DefinedName> {
        self.model
            .get_defined_name_list()
            .into_iter()
            .map(|(name, scope, formula)| DefinedName {
                name,
                scope,
                formula: formula.trim_start_matches('=').to_owned(),
            })
            .collect()
    }

    /// Define `name` (on sheet `scope`, else the workbook) as `formula`
    /// (`Data!$A$1:$A$10`, `0.11`...), replacing a name of that scope.
    pub fn set_defined_name(
        &mut self,
        name: &str,
        scope: Option<u32>,
        formula: &str,
    ) -> Result<()> {
        let formula = formula.trim().trim_start_matches('=');
        if formula.is_empty() {
            bail!("a name needs what it refers to");
        }
        let exists = self
            .defined_names()
            .iter()
            .any(|n| n.scope == scope && n.name.eq_ignore_ascii_case(name));
        let result = if exists {
            self.model
                .update_defined_name(name, scope, name, scope, formula)
        } else {
            self.model.new_defined_name(name, scope, formula)
        };
        result.map_err(|e| anyhow!("can't define {name}: {e}"))?;
        self.history.edited();
        Ok(())
    }

    /// Remove defined name `name` of `scope`.
    pub fn delete_defined_name(&mut self, name: &str, scope: Option<u32>) -> Result<()> {
        let Some(found) = self
            .defined_names()
            .into_iter()
            .find(|n| n.scope == scope && n.name.eq_ignore_ascii_case(name))
        else {
            bail!("there is no name {name}");
        };
        self.model
            .delete_defined_name(&found.name, scope)
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(())
    }

    /// The pivot tables on `sheet`, where they are now.
    pub fn pivots(&self, sheet: u32) -> Vec<crate::PivotInfo> {
        self.placed_pivots(sheet)
            .iter()
            .map(|p| self.pivot_info(p))
            .collect()
    }

    /// Whether a cell's number format shows a date or time.
    pub fn is_date_format(&self, sheet: u32, row: u32, col: u32) -> bool {
        self.number_format_by_index(self.style_index(sheet, row, col))
            .is_date()
    }

    /// Put a chart whose `c:chartSpace` part is `xml` (references to this
    /// workbook's cells, in the current rows and columns) on `sheet` at
    /// `anchor`. `spec` is what the Chart panel shows for it (its type and
    /// data block). Returns its key.
    pub fn insert_chart_part(
        &mut self,
        sheet: u32,
        anchor: Anchor,
        xml: String,
        spec: ChartSpec,
    ) -> Result<DrawingKey> {
        if sheet as usize >= self.sheet_names().len() {
            bail!("there is no sheet {}", sheet + 1);
        }
        roxmltree::Document::parse(&xml).map_err(|e| anyhow!("the chart part: {e}"))?;
        let sheet_id = self.sheet_id(sheet);
        let mut id = 0;
        self.own_edit(|e, layout| {
            id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
            e.charts.push(NewChart {
                id,
                sheet_id,
                spec,
                anchor,
                layout: layout.clone(),
                xml: Some(xml.into()),
                picture: None,
                shape: None,
                links: None,
            });
        });
        Ok(DrawingKey {
            sheet_id,
            index: NEW_CHART_BASE + id as usize,
        })
    }
}
