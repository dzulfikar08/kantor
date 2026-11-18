//! Section columns (`w:sectPr/w:cols`): how many, their widths and the
//! space between them, and the line between them, as Word's Columns
//! dialog sets them.
//!
//! - `w:num` columns of equal width `w:space` apart, unless `w:equalWidth`
//!   is off: then each `w:col` gives its own width and the space after it.
//! - `w:sep` draws a line between the columns.
//!
//! Layout flows text down the first column, then the next (see
//! `docs_layout`); a column break ([`COLUMN_BREAK`]) moves on to the next
//! column. Changing columns writes only the section's `w:cols` (and, for
//! "This point forward" or selected text, the continuous section breaks
//! Word adds); everything else in the section properties is kept.

use std::collections::BTreeMap;

use crate::document::Document;
use crate::model::{Paragraph, Selection};
use crate::page::{sect_prefix, set_sect_child};
use crate::section::{SectAt, para_sect};
use crate::xml;

/// Column break inside a paragraph (`w:br w:type="column"`). Word's own
/// character for it in `.doc` files.
pub const COLUMN_BREAK: char = '\u{000E}';

/// Word's default space between columns: 1.27 cm (0.5 in).
pub const DEFAULT_SPACE: u32 = 720;

/// Word allows at most 45 columns.
pub const MAX_COLUMNS: u16 = 45;

/// One column of unequal ones: its width and the space after it, twips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub width: u32,
    pub space: u32,
}

/// A section's columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Columns {
    /// Number of columns (1: none).
    pub num: u16,
    /// Space between equal columns, twips.
    pub space: u32,
    /// All columns the same width (`w:equalWidth`).
    pub equal: bool,
    /// A line between columns (`w:sep`).
    pub sep: bool,
    /// Each column's width and the space after it (unequal columns).
    pub cols: Vec<Column>,
}

impl Default for Columns {
    fn default() -> Self {
        Self::one()
    }
}

/// Word's Columns presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnPreset {
    One,
    Two,
    Three,
    /// Two columns, the left one narrower.
    Left,
    /// Two columns, the right one narrower.
    Right,
}

impl ColumnPreset {
    pub const ALL: [Self; 5] = [Self::One, Self::Two, Self::Three, Self::Left, Self::Right];

    pub fn label(self) -> &'static str {
        match self {
            Self::One => "One",
            Self::Two => "Two",
            Self::Three => "Three",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }

    /// The preset matching `c` (on a text area `text` twips wide).
    pub fn of(c: &Columns, text: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|p| {
            let mut want = p.columns(text);
            want.sep = c.sep;
            want.same_layout(c, text)
        })
    }

    /// The columns this preset makes on a text area `text` twips wide.
    pub fn columns(self, text: u32) -> Columns {
        match self {
            Self::One => Columns::one(),
            Self::Two => Columns::equal(2, DEFAULT_SPACE),
            Self::Three => Columns::equal(3, DEFAULT_SPACE),
            Self::Left | Self::Right => {
                // Word's Left/Right: the narrow column a bit under half the
                // wide one (1.83" beside 4.17" on a 6.5" text area).
                let room = text.saturating_sub(DEFAULT_SPACE);
                let narrow = (room as f64 * 0.305).round() as u32;
                let wide = room - narrow;
                let (a, b) = if self == Self::Left {
                    (narrow, wide)
                } else {
                    (wide, narrow)
                };
                Columns {
                    num: 2,
                    space: DEFAULT_SPACE,
                    equal: false,
                    sep: false,
                    cols: vec![
                        Column {
                            width: a,
                            space: DEFAULT_SPACE,
                        },
                        Column { width: b, space: 0 },
                    ],
                }
            }
        }
    }
}

/// Where the Columns dialog's settings go (its "Apply to").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnsApply {
    WholeDocument,
    ThisSection,
    /// From the caret's paragraph on: a continuous section break is added
    /// before it.
    ThisPointForward,
    /// The selected paragraphs: continuous section breaks around them.
    SelectedText,
}

impl ColumnsApply {
    pub fn label(self) -> &'static str {
        match self {
            Self::WholeDocument => "Whole document",
            Self::ThisSection => "This section",
            Self::ThisPointForward => "This point forward",
            Self::SelectedText => "Selected text",
        }
    }
}

impl Columns {
    /// A single column.
    pub fn one() -> Self {
        Self {
            num: 1,
            space: DEFAULT_SPACE,
            equal: true,
            sep: false,
            cols: Vec::new(),
        }
    }

    /// `num` equal columns `space` twips apart.
    pub fn equal(num: u16, space: u32) -> Self {
        Self {
            num: num.clamp(1, MAX_COLUMNS),
            space,
            equal: true,
            sep: false,
            cols: Vec::new(),
        }
    }

    /// How many columns there are.
    pub fn count(&self) -> usize {
        if self.equal || self.cols.is_empty() {
            self.num.max(1) as usize
        } else {
            self.cols.len()
        }
    }

    /// Read from a `w:cols` element.
    pub(crate) fn from_el(c: &xml::El) -> Self {
        let cols: Vec<Column> = c
            .children
            .iter()
            .filter(|k| k.name == "col")
            .map(|k| Column {
                width: k.num::<i64>("w").unwrap_or(0).max(0) as u32,
                space: k.num::<i64>("space").unwrap_or(0).max(0) as u32,
            })
            .collect();
        let flag = |name: &str| c.attr(name).map(|v| !matches!(v, "0" | "false" | "off"));
        // Without `w:equalWidth`, `w:col` children mean their own widths.
        let equal = flag("equalWidth").unwrap_or(cols.is_empty()) || cols.is_empty();
        let num = c
            .num::<i64>("num")
            .map(|n| n.clamp(1, MAX_COLUMNS as i64) as u16)
            .unwrap_or(if equal { 1 } else { cols.len() as u16 });
        Self {
            num: if equal { num } else { cols.len() as u16 },
            space: c
                .num::<i64>("space")
                .map_or(DEFAULT_SPACE, |s| s.max(0) as u32),
            equal,
            sep: flag("sep").unwrap_or(false),
            cols: if equal { Vec::new() } else { cols },
        }
    }

    /// Read from a whole `w:sectPr`.
    pub fn parse(sect_pr: &str) -> Self {
        xml::parse(sect_pr)
            .and_then(|el| el.child("cols").map(Self::from_el))
            .unwrap_or_default()
    }

    /// Each column's left edge (from the text area's left) and width on a
    /// text area `text` twips wide. Equal columns share what the spaces
    /// leave; unequal ones are as the file says, made narrower in
    /// proportion when they'd be wider than the text area.
    pub fn layout(&self, text: u32) -> Vec<(u32, u32)> {
        let n = self.count();
        if n <= 1 {
            return vec![(0, text)];
        }
        if self.equal || self.cols.is_empty() {
            let gaps = self.space as u64 * (n as u64 - 1);
            // Spaces never leave a column under a tenth of its share.
            let gaps = gaps.min(text as u64 * 9 / 10) as u32;
            let space = gaps / (n as u32 - 1);
            let w = (text - gaps) / n as u32;
            return (0..n as u32).map(|i| (i * (w + space), w)).collect();
        }
        let total: u64 = self
            .cols
            .iter()
            .enumerate()
            .map(|(i, c)| c.width as u64 + if i + 1 < n { c.space as u64 } else { 0 })
            .sum();
        let scale = if total > text as u64 && total > 0 {
            text as f64 / total as f64
        } else {
            1.0
        };
        let mut x = 0f64;
        self.cols
            .iter()
            .map(|c| {
                let at = (
                    x.round() as u32,
                    ((c.width as f64 * scale).round() as u32).max(1),
                );
                x += (c.width as f64 + c.space as f64) * scale;
                at
            })
            .collect()
    }

    /// Same columns on a text area `text` twips wide (rounding aside),
    /// line between ignored.
    pub fn same_layout(&self, other: &Columns, text: u32) -> bool {
        let (a, b) = (self.layout(text), other.layout(text));
        a.len() == b.len()
            && a.iter()
                .zip(&b)
                .all(|(x, y)| x.0.abs_diff(y.0) <= 20 && x.1.abs_diff(y.1) <= 20)
    }

    /// The `w:cols` element, written with prefix `p` (`w:`) as Word
    /// writes it.
    pub fn to_xml(&self, p: &str) -> String {
        let n = self.count();
        let sep = if self.sep && n > 1 {
            format!(" {p}sep=\"1\"")
        } else {
            String::new()
        };
        if n <= 1 {
            return format!("<{p}cols {p}space=\"{}\"/>", self.space);
        }
        if self.equal || self.cols.is_empty() {
            return format!("<{p}cols {p}num=\"{n}\"{sep} {p}space=\"{}\"/>", self.space);
        }
        let mut x = format!(
            "<{p}cols {p}num=\"{n}\"{sep} {p}space=\"{}\" {p}equalWidth=\"0\">",
            self.space
        );
        for (i, c) in self.cols.iter().enumerate() {
            if i + 1 < n {
                x.push_str(&format!(
                    "<{p}col {p}w=\"{}\" {p}space=\"{}\"/>",
                    c.width, c.space
                ));
            } else {
                x.push_str(&format!("<{p}col {p}w=\"{}\"/>", c.width));
            }
        }
        x.push_str(&format!("</{p}cols>"));
        x
    }

    /// These columns as unequal ones (each its own width and spacing) on a
    /// text area `text` twips wide: the dialog's starting point when
    /// "Equal column width" is turned off.
    pub fn to_unequal(&self, text: u32) -> Self {
        let lay = self.layout(text);
        let n = lay.len();
        let cols = (0..n)
            .map(|i| Column {
                width: lay[i].1,
                space: lay
                    .get(i + 1)
                    .map_or(0, |next| next.0.saturating_sub(lay[i].0 + lay[i].1)),
            })
            .collect();
        Self {
            num: n as u16,
            equal: false,
            cols,
            ..self.clone()
        }
    }
}

/// An edge of the space between two columns, as the ruler shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnEdge {
    /// The right edge of column `.0` (the space after it starts here).
    End(usize),
    /// The left edge of column `.0 + 1` (the space after column `.0`
    /// ends here).
    Start(usize),
}

/// A ruler drag never makes a column narrower than this: 0.5 cm, twips.
pub const MIN_COLUMN: u32 = 283;

impl Columns {
    /// Where an edge sits, twips from the text area's left (`None`: no
    /// such edge).
    pub fn edge_at(&self, text: u32, edge: ColumnEdge) -> Option<u32> {
        let lay = self.layout(text);
        match edge {
            ColumnEdge::End(i) if i + 1 < lay.len() => Some(lay[i].0 + lay[i].1),
            ColumnEdge::Start(i) if i + 1 < lay.len() => Some(lay[i + 1].0),
            _ => None,
        }
    }

    /// These columns with `edge` dragged to `at` (twips from the text
    /// area's left), as Word's ruler: equal columns stay equal (every
    /// space and width changes to put the edge there); with unequal ones
    /// the edge's column and the space beside it change, the other
    /// columns stay. `None` when nothing changes.
    pub fn dragged(&self, text: u32, edge: ColumnEdge, at: u32) -> Option<Self> {
        let lay = self.layout(text);
        let n = lay.len();
        let old = self.edge_at(text, edge)?;
        if old == at {
            return None;
        }
        let t = text as f64;
        let x = at as f64;
        if self.equal || self.cols.is_empty() {
            let nf = n as f64;
            let (w, s) = match edge {
                ColumnEdge::End(i) => {
                    // x = (i+1)w + i s and t = n w + (n-1) s.
                    let (a, b) = ((i + 1) as f64, i as f64);
                    let det = a * (nf - 1.0) - b * nf;
                    ((x * (nf - 1.0) - b * t) / det, (a * t - nf * x) / det)
                }
                ColumnEdge::Start(i) => {
                    // x = (i+1)(w + s) and t = n w + (n-1) s.
                    let p = x / (i + 1) as f64;
                    let s = nf * p - t;
                    (p - s, s)
                }
            };
            if w < MIN_COLUMN as f64 || s < 0.0 {
                return None;
            }
            let space = s.round() as u32;
            return (space != self.space).then(|| Self {
                space,
                ..self.clone()
            });
        }
        let mut cols: Vec<Column> = (0..n)
            .map(|i| Column {
                width: lay[i].1,
                space: lay
                    .get(i + 1)
                    .map_or(0, |next| next.0.saturating_sub(lay[i].0 + lay[i].1)),
            })
            .collect();
        match edge {
            ColumnEdge::End(i) => {
                let next = lay[i + 1].0;
                let at = at.clamp(lay[i].0 + MIN_COLUMN, next);
                cols[i].width = at - lay[i].0;
                cols[i].space = next - at;
            }
            ColumnEdge::Start(i) => {
                let end = lay[i].0 + lay[i].1;
                let right = lay[i + 1].0 + lay[i + 1].1;
                let at = at.clamp(end, right.saturating_sub(MIN_COLUMN).max(end));
                cols[i].space = at - end;
                cols[i + 1].width = right - at;
            }
        }
        let out = Self {
            num: n as u16,
            equal: false,
            cols,
            ..self.clone()
        };
        (out != *self).then_some(out)
    }
}

impl Document {
    /// The columns of the section holding paragraph `para`.
    pub fn section_columns(&self, para: usize) -> Columns {
        let sections = self.sections();
        sections[self.section_of(para)].props.columns.clone()
    }

    /// Text width (twips) of the section holding paragraph `para`.
    pub fn section_text_width(&self, para: usize) -> u32 {
        let g = self.section_page(para);
        g.width
            .saturating_sub(g.margin_left + g.margin_right)
            .max(567)
    }

    /// Insert a column break at the caret (replacing a selection).
    pub fn insert_column_break(&mut self, sel: Selection) -> Selection {
        self.insert_text(sel, &COLUMN_BREAK.to_string(), None)
    }

    /// Set columns as Word's Columns dialog does, `apply` saying where.
    /// "This point forward" and selected text add continuous section
    /// breaks (falling back to the caret's section where a break can't go,
    /// e.g. right after a table). One undo step; returns the selection to
    /// keep.
    pub fn set_columns(
        &mut self,
        sel: Selection,
        cols: &Columns,
        apply: ColumnsApply,
    ) -> Selection {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let before = self.side_state();
        let mut old: BTreeMap<usize, Paragraph> = BTreeMap::new();
        let first_si = self.section_of(start.para);
        // Sections (by index after any breaks are added) to change.
        let targets: Vec<usize> = match apply {
            ColumnsApply::WholeDocument => (0..self.sections().len()).collect(),
            ColumnsApply::ThisSection => vec![first_si],
            ColumnsApply::ThisPointForward => {
                let si = match self.split_section_before(start.para, &mut old) {
                    true => first_si + 1,
                    false => first_si,
                };
                (si..self.sections().len()).collect()
            }
            ColumnsApply::SelectedText => {
                // The part after the selection first, so `start` stays put.
                let after = end.para + 1;
                let split_end = after < self.paragraphs.len()
                    && self.section_of(after) == self.section_of(end.para)
                    && self.split_section_before(after, &mut old);
                let split_start = self.split_section_before(start.para, &mut old);
                let si = first_si + split_start as usize;
                if split_end {
                    // The text after the selection goes on on the same page.
                    self.set_section_start_continuous(si + 1, &mut old);
                }
                vec![si]
            }
        };
        for si in targets {
            self.set_section_cols(si, cols, &mut old);
        }
        let after = self.side_state();
        let (Some(&a), Some(&b)) = (old.keys().next(), old.keys().next_back()) else {
            if before != after {
                self.push_command(
                    0,
                    Vec::new(),
                    Vec::new(),
                    sel,
                    Some(Box::new((before, after))),
                );
            }
            return sel;
        };
        let olds: Vec<Paragraph> = (a..=b)
            .map(|i| {
                old.get(&i)
                    .cloned()
                    .unwrap_or_else(|| self.paragraphs[i].clone())
            })
            .collect();
        let news: Vec<Paragraph> = self.paragraphs[a..=b].to_vec();
        let hf = (before != after).then(|| Box::new((before, after)));
        self.push_command(a, olds, news, sel, hf);
        sel
    }

    /// The section properties XML of section `si` (the body's with the
    /// model's page geometry written in).
    fn sect_xml_full(&self, si: usize) -> String {
        let sections = self.sections();
        let xml = self.sect_xml(&sections, si);
        if sections[si].at == SectAt::Body {
            crate::page::update_sect_pr(&xml, &self.page)
        } else {
            xml
        }
    }

    /// Replace section `si`'s properties XML, remembering a paragraph's
    /// old value in `old`.
    fn put_sect_xml(&mut self, si: usize, xml: String, old: &mut BTreeMap<usize, Paragraph>) {
        let sections = self.sections();
        if let SectAt::Para(i) = sections[si].at {
            old.entry(i).or_insert_with(|| self.paragraphs[i].clone());
        }
        self.set_sect_xml(&sections, si, xml);
    }

    fn set_section_cols(
        &mut self,
        si: usize,
        cols: &Columns,
        old: &mut BTreeMap<usize, Paragraph>,
    ) {
        let sections = self.sections();
        if sections[si].props.columns == *cols {
            return;
        }
        let xml = self.sect_xml(&sections, si);
        let p = sect_prefix(&xml).to_owned();
        let xml = set_sect_child(&xml, "cols", |_| true, Some(cols.to_xml(&p)));
        self.put_sect_xml(si, xml, old);
    }

    fn set_section_start_continuous(&mut self, si: usize, old: &mut BTreeMap<usize, Paragraph>) {
        let sections = self.sections();
        if si >= sections.len() || sections[si].props.start == crate::SectionStart::Continuous {
            return;
        }
        let xml = self.sect_xml(&sections, si);
        let p = sect_prefix(&xml).to_owned();
        let xml = set_sect_child(
            &xml,
            "type",
            |_| true,
            Some(format!("<{p}type {p}val=\"continuous\"/>")),
        );
        self.put_sect_xml(si, xml, old);
    }

    /// Make paragraph `para` the first of a new section that starts
    /// continuously (Word's "Section Break (Continuous)" before it): the
    /// paragraph before it ends a section with the old section's settings.
    /// `false` when `para` already starts a section or the paragraph
    /// before it can't hold a break (it's in a table).
    fn split_section_before(&mut self, para: usize, old: &mut BTreeMap<usize, Paragraph>) -> bool {
        if para == 0 || para >= self.paragraphs.len() {
            return false;
        }
        let sections = self.sections();
        let si = self.section_of(para);
        if sections[si].paras.start == para
            || self.paragraphs[para - 1].cell.is_some()
            || self.paragraphs[para].cell.is_some()
            || para_sect(&self.paragraphs[para - 1]).is_some()
        {
            return false;
        }
        let xml = self.sect_xml_full(si);
        old.entry(para - 1)
            .or_insert_with(|| self.paragraphs[para - 1].clone());
        self.paragraphs[para - 1].props.extras.push(&xml);
        // The section now at `si + 1` is the rest: it starts on the page.
        self.set_section_start_continuous(si + 1, old);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Position;

    #[std::prelude::v1::test]
    fn reads_equal_and_unequal_columns() {
        let c =
            Columns::parse(r#"<w:sectPr><w:cols w:num="2" w:space="708" w:sep="1"/></w:sectPr>"#);
        assert_eq!((c.count(), c.space, c.sep, c.equal), (2, 708, true, true));
        assert_eq!(c.layout(9000), vec![(0, 4146), (4854, 4146)]);
        let u = Columns::parse(
            r#"<w:sectPr><w:cols w:num="2" w:space="720" w:equalWidth="0"><w:col w:w="2000" w:space="500"/><w:col w:w="6000"/></w:cols></w:sectPr>"#,
        );
        assert_eq!(u.count(), 2);
        assert!(!u.equal);
        assert_eq!(u.layout(9000), vec![(0, 2000), (2500, 6000)]);
        // Too wide for the text area: narrower in proportion.
        assert_eq!(u.layout(4250), vec![(0, 1000), (1250, 3000)]);
        assert_eq!(Columns::parse("<w:sectPr/>"), Columns::one());
        assert_eq!(
            Columns::parse(r#"<w:sectPr><w:cols w:space="720"/></w:sectPr>"#).count(),
            1
        );
    }

    #[std::prelude::v1::test]
    fn ruler_drags_change_widths_and_spacing() {
        // Equal columns stay equal: dragging the first column's right
        // edge left widens the space, every column narrower.
        let c = Columns::equal(2, 720);
        assert_eq!(c.edge_at(9000, ColumnEdge::End(0)), Some(4140));
        let d = c.dragged(9000, ColumnEdge::End(0), 4000).unwrap();
        assert!(d.equal);
        assert_eq!(d.space, 1000);
        assert_eq!(d.layout(9000), vec![(0, 4000), (5000, 4000)]);
        let d = c.dragged(9000, ColumnEdge::Start(0), 4600).unwrap();
        assert_eq!(d.layout(9000), vec![(0, 4400), (4600, 4400)]);
        let three = Columns::equal(3, 720);
        let d = three.dragged(9000, ColumnEdge::End(1), 5800).unwrap();
        let lay = d.layout(9000);
        assert_eq!(lay[1].0 + lay[1].1, 5800);
        assert!(lay.iter().all(|c| c.1 == lay[0].1));
        // No room left: refused; no move: nothing.
        assert_eq!(c.dragged(9000, ColumnEdge::End(0), 100), None);
        assert_eq!(c.dragged(9000, ColumnEdge::End(0), 4140), None);
        assert_eq!(c.dragged(9000, ColumnEdge::End(1), 100), None);
        // Unequal: the edge's column and the space beside it.
        let u = ColumnPreset::Left.columns(9000);
        let lay = u.layout(9000);
        let d = u.dragged(9000, ColumnEdge::End(0), 2000).unwrap();
        assert_eq!(d.layout(9000), vec![(0, 2000), lay[1]]);
        let d = u.dragged(9000, ColumnEdge::Start(0), 4000).unwrap();
        assert_eq!(d.layout(9000), vec![lay[0], (4000, 5000)]);
        // An edge never crosses the space's other edge.
        let d = u.dragged(9000, ColumnEdge::End(0), 8000).unwrap();
        assert_eq!(d.layout(9000), vec![(0, lay[1].0), lay[1]]);
    }

    #[std::prelude::v1::test]
    fn writes_what_it_reads() {
        for c in [
            Columns::one(),
            Columns::equal(3, 567),
            Columns {
                sep: true,
                ..ColumnPreset::Left.columns(9000)
            },
        ] {
            let x = format!("<w:sectPr>{}</w:sectPr>", c.to_xml("w:"));
            assert_eq!(Columns::parse(&x), c, "{x}");
        }
        assert_eq!(
            ColumnPreset::Right.columns(9360).layout(9360)[1].1,
            2635,
            "the narrow column is on the right"
        );
        assert_eq!(
            ColumnPreset::of(&Columns::equal(2, 720), 9000),
            Some(ColumnPreset::Two)
        );
    }

    fn doc(n: usize) -> Document {
        let mut d = Document::new();
        let text: Vec<String> = (0..n).map(|i| format!("Paragraph {i}")).collect();
        d.insert_text(
            Selection::caret(Position::new(0, 0)),
            &text.join("\n"),
            None,
        );
        d
    }

    #[std::prelude::v1::test]
    fn columns_apply_to_sections_in_one_undo_step() {
        let mut d = doc(6);
        let caret = Selection::caret(Position::new(3, 0));
        d.set_columns(caret, &Columns::equal(2, 720), ColumnsApply::WholeDocument);
        assert_eq!(d.section_columns(0).count(), 2);
        assert_eq!(d.sections().len(), 1);
        d.undo();
        assert_eq!(d.section_columns(0).count(), 1);

        // This point forward: a continuous break before paragraph 3.
        d.set_columns(
            caret,
            &Columns::equal(3, 720),
            ColumnsApply::ThisPointForward,
        );
        let s = d.sections();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].paras, 0..3);
        assert_eq!(s[0].props.columns.count(), 1);
        assert_eq!(s[1].props.columns.count(), 3);
        assert_eq!(s[1].props.start, crate::SectionStart::Continuous);
        d.undo();
        assert_eq!(d.sections().len(), 1);
        assert_eq!(d.section_columns(5).count(), 1);

        // Selected text: breaks on both sides.
        let sel = Selection {
            anchor: Position::new(2, 0),
            head: Position::new(3, 2),
        };
        d.set_columns(sel, &Columns::equal(2, 720), ColumnsApply::SelectedText);
        let s = d.sections();
        assert_eq!(s.len(), 3);
        assert_eq!(s[1].paras, 2..4);
        assert_eq!(
            s.iter()
                .map(|s| s.props.columns.count())
                .collect::<Vec<_>>(),
            vec![1, 2, 1]
        );
        assert_eq!(s[2].props.start, crate::SectionStart::Continuous);
        d.undo();
        assert_eq!(d.sections().len(), 1);
    }

    #[std::prelude::v1::test]
    fn column_break_round_trips() {
        let mut d = doc(1);
        d.insert_column_break(Selection::caret(Position::new(0, 4)));
        assert!(d.paragraphs()[0].text.contains(COLUMN_BREAK));
        d.set_columns(
            Selection::caret(Position::new(0, 0)),
            &Columns::equal(2, 720),
            ColumnsApply::ThisSection,
        );
        let dir = std::env::temp_dir().join(format!("kantor-cols-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cols.docx");
        d.save_as(&path).unwrap();
        let back = Document::open(&path).unwrap();
        assert_eq!(back.paragraphs()[0].text, d.paragraphs()[0].text);
        assert_eq!(back.section_columns(0), Columns::equal(2, 720));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
