//! Table Properties: alignment between the margins (`w:jc`) and indent
//! from left (`w:tblInd`), as Word's Table Properties › Table tab.

use crate::document::{Document, Kind};
use crate::model::Selection;
use crate::page::{child_tags, set_child_ranked};
use crate::table::TableAlign;

/// `w:tblPr` children in schema order.
const TBLPR_ORDER: &[&str] = &[
    "tblStyle",
    "tblpPr",
    "tblOverlap",
    "bidiVisual",
    "tblStyleRowBandSize",
    "tblStyleColBandSize",
    "tblW",
    "jc",
    "tblCellSpacing",
    "tblInd",
    "tblBorders",
    "shd",
    "tblLayout",
    "tblCellMar",
    "tblLook",
    "tblCaption",
    "tblDescription",
    "tblPrChange",
];

fn rank(name: &str) -> usize {
    TBLPR_ORDER
        .iter()
        .position(|n| *n == name)
        .unwrap_or(TBLPR_ORDER.len())
}

/// `tbl_pr` with child `name` set to `new` (or removed), in schema order;
/// every other byte kept.
pub(crate) fn set_tbl_pr_child(tbl_pr: &str, name: &str, new: Option<String>) -> String {
    set_child_ranked(tbl_pr, name, rank, |_| true, new)
}

/// What the Table Properties dialog shows and sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableProps {
    pub align: TableAlign,
    /// Indent from left in twips (used when left-aligned).
    pub indent: i32,
}

impl Document {
    /// The properties of the table at the caret.
    pub fn table_props(&self, sel: Selection) -> Option<TableProps> {
        let at = self.cell_at(self.clamp_selection(sel).head)?;
        let t = self.tables.get(&at.table)?;
        let style = self.styles.table_style(t.style.as_deref());
        Some(TableProps {
            align: t.align.or(style.align).unwrap_or_default(),
            indent: t.indent,
        })
    }

    /// Set the alignment and indent of the table at the caret (one undo
    /// step; tracked as a table property change with Track Changes on).
    /// The indent is dropped for centred and right-aligned tables, as
    /// Word's dialog disables it then.
    pub fn set_table_props(&mut self, sel: Selection, props: TableProps) -> Result<(), String> {
        let sel = self.clamp_selection(sel);
        let Some(at) = self.cell_at(sel.head) else {
            return Err("The caret isn't in a table".into());
        };
        let id = at.table;
        let mut t = self.tables[&id].clone();
        let old = t.tbl_pr.clone();
        let base = old
            .as_deref()
            .map(own_props)
            .unwrap_or_else(|| "<w:tblPr></w:tblPr>".to_owned());
        let p = prefix(&base);
        let mut x = set_tbl_pr_child(
            &base,
            "jc",
            Some(format!("<{p}jc {p}val=\"{}\"/>", props.align.jc())),
        );
        let indent = (props.align == TableAlign::Left && props.indent != 0)
            .then(|| format!("<{p}tblInd {p}w=\"{}\" {p}type=\"dxa\"/>", props.indent));
        x = set_tbl_pr_child(&x, "tblInd", indent);
        // Word writes `w:jc` only when it isn't the default: keep a file's
        // explicit value, but a table that had none and stays left gets none.
        if props.align == TableAlign::Left
            && !base.contains(&format!("<{p}jc "))
            && self.styles.table_style(t.style.as_deref()).align.is_none()
        {
            x = set_tbl_pr_child(&x, "jc", None);
        }
        let x = match old.as_deref().and_then(record_of) {
            // An earlier record stays: it holds the properties before any change.
            Some(rec) => set_tbl_pr_child(&x, "tblPrChange", Some(rec.to_owned())),
            None => self.track_props(old.as_deref(), x, "tblPr"),
        };
        if old.as_deref() == Some(x.as_str()) {
            return Ok(());
        }
        t.tbl_pr = Some(x.into());
        t.refresh();
        let mut tables = self.tables.clone();
        tables.insert(id, t);
        let start = self.table_range(id).start;
        self.replace_with_tables(
            start,
            0,
            Vec::new(),
            Some(tables),
            (sel, sel),
            Kind::Other,
            true,
        );
        Ok(())
    }
}

/// `tbl_pr` without its `w:tblPrChange`.
fn own_props(tbl_pr: &str) -> String {
    set_tbl_pr_child(tbl_pr, "tblPrChange", None)
}

/// The `w:tblPrChange` of `tbl_pr`, as written.
fn record_of(tbl_pr: &str) -> Option<&str> {
    let t = child_tags(tbl_pr)
        .into_iter()
        .find(|t| t.name == "tblPrChange")?;
    let r = crate::float::element_range(&tbl_pr[t.start..], "tblPrChange")?;
    Some(&tbl_pr[t.start + r.start..t.start + r.end])
}

/// The root element's prefix with its colon (`w:`), or none.
fn prefix(xml: &str) -> &str {
    let s = xml.find('<').map_or(0, |i| i + 1);
    let end = xml[s..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .map_or(xml.len(), |e| s + e);
    match xml[s..end].find(':') {
        Some(c) => &xml[s..s + c + 1],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_children_in_schema_order() {
        let x = "<w:tblPr><w:tblStyle w:val=\"T\"/><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblLook w:val=\"04A0\"/></w:tblPr>";
        let y = set_tbl_pr_child(x, "jc", Some("<w:jc w:val=\"center\"/>".into()));
        assert_eq!(
            y,
            "<w:tblPr><w:tblStyle w:val=\"T\"/><w:tblW w:w=\"0\" w:type=\"auto\"/><w:jc w:val=\"center\"/><w:tblLook w:val=\"04A0\"/></w:tblPr>"
        );
        let z = set_tbl_pr_child(
            &y,
            "tblInd",
            Some("<w:tblInd w:w=\"5\" w:type=\"dxa\"/>".into()),
        );
        assert!(z.contains("<w:jc w:val=\"center\"/><w:tblInd"));
        assert_eq!(set_tbl_pr_child(&y, "jc", None), x);
        assert_eq!(
            set_tbl_pr_child("<w:tblPr/>", "jc", Some("<w:jc w:val=\"right\"/>".into())),
            "<w:tblPr><w:jc w:val=\"right\"/></w:tblPr>"
        );
    }
}
