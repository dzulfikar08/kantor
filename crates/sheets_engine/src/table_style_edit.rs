//! Custom table styles, as Excel's Table Design ▸ New Table Style… makes
//! them: a name, a format for each table element (whole table, stripes,
//! first and last column, header and total rows, their first and last
//! cells), stripe sizes, and "Set as default table style for this
//! document"; Modify, Duplicate and Delete from the style gallery.
//!
//! The opened file's `<tableStyles>` are read by `table::parse_custom_styles`
//! into [`FileStyles`] (each style's markup and dxfs as the file has them).
//! An edit makes a new history state whose [`crate::edits::Edits`] hold the
//! whole set ([`StyleSet`]): undo and redo swap sets. A save writes
//! `<tableStyles>` into the styles part ([`patch_styles`]): styles from the
//! file keep their markup and dxfs byte for byte, styles made or changed in
//! Kantor get dxfs appended after the model's; a file whose styles nobody
//! touched gets its `<tableStyles>` element back as it was.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::format::{BorderLine, Rgb};
use crate::table::{Elem, TableStyleDef};
use crate::table::{TableCellLook, TableInfo};
use crate::{CellRange, Workbook};

/// Excel's default style for new tables.
pub const DEFAULT_TABLE_STYLE: &str = "TableStyleMedium2";

/// The elements of a table style, in the order Excel lists (and draws, one
/// over the other) them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableElement {
    WholeTable,
    FirstColumnStripe,
    SecondColumnStripe,
    FirstRowStripe,
    SecondRowStripe,
    LastColumn,
    FirstColumn,
    HeaderRow,
    TotalRow,
    FirstHeaderCell,
    LastHeaderCell,
    FirstTotalCell,
    LastTotalCell,
}

impl TableElement {
    pub const ALL: [TableElement; 13] = [
        Self::WholeTable,
        Self::FirstColumnStripe,
        Self::SecondColumnStripe,
        Self::FirstRowStripe,
        Self::SecondRowStripe,
        Self::LastColumn,
        Self::FirstColumn,
        Self::HeaderRow,
        Self::TotalRow,
        Self::FirstHeaderCell,
        Self::LastHeaderCell,
        Self::FirstTotalCell,
        Self::LastTotalCell,
    ];

    /// The OOXML `tableStyleElement` type.
    pub fn xml(self) -> &'static str {
        match self {
            Self::WholeTable => "wholeTable",
            Self::FirstColumnStripe => "firstColumnStripe",
            Self::SecondColumnStripe => "secondColumnStripe",
            Self::FirstRowStripe => "firstRowStripe",
            Self::SecondRowStripe => "secondRowStripe",
            Self::LastColumn => "lastColumn",
            Self::FirstColumn => "firstColumn",
            Self::HeaderRow => "headerRow",
            Self::TotalRow => "totalRow",
            Self::FirstHeaderCell => "firstHeaderCell",
            Self::LastHeaderCell => "lastHeaderCell",
            Self::FirstTotalCell => "firstTotalCell",
            Self::LastTotalCell => "lastTotalCell",
        }
    }

    /// Excel's name for it (English; the UI translates).
    pub fn label(self) -> &'static str {
        match self {
            Self::WholeTable => "Whole Table",
            Self::FirstColumnStripe => "First Column Stripe",
            Self::SecondColumnStripe => "Second Column Stripe",
            Self::FirstRowStripe => "First Row Stripe",
            Self::SecondRowStripe => "Second Row Stripe",
            Self::LastColumn => "Last Column",
            Self::FirstColumn => "First Column",
            Self::HeaderRow => "Header Row",
            Self::TotalRow => "Total Row",
            Self::FirstHeaderCell => "First Header Cell",
            Self::LastHeaderCell => "Last Header Cell",
            Self::FirstTotalCell => "First Total Cell",
            Self::LastTotalCell => "Last Total Cell",
        }
    }

    /// Stripes have a size (rows or columns in each stripe, 1 to 9).
    pub fn has_size(self) -> bool {
        matches!(
            self,
            Self::FirstColumnStripe
                | Self::SecondColumnStripe
                | Self::FirstRowStripe
                | Self::SecondRowStripe
        )
    }
}

/// A table style as New Table Style / Modify Table Style shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TableStyleSpec {
    pub name: String,
    /// The format of each element (an empty [`Elem`]: not formatted).
    pub formats: Vec<(TableElement, Elem)>,
    /// Stripe sizes, by element (1 when missing).
    pub sizes: Vec<(TableElement, u32)>,
}

impl TableStyleSpec {
    /// A style named `name` with nothing formatted.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            formats: Vec::new(),
            sizes: Vec::new(),
        }
    }

    /// The format of element `e`.
    pub fn format(&self, e: TableElement) -> Elem {
        self.formats
            .iter()
            .find(|(k, _)| *k == e)
            .map(|(_, f)| f.clone())
            .unwrap_or_default()
    }

    pub fn set_format(&mut self, e: TableElement, f: Elem) {
        self.formats.retain(|(k, _)| *k != e);
        if !f.is_empty() {
            self.formats.push((e, f));
        }
    }

    /// The stripe size of element `e`.
    pub fn size(&self, e: TableElement) -> u32 {
        self.sizes
            .iter()
            .find(|(k, _)| *k == e)
            .map_or(1, |(_, s)| *s)
    }

    pub fn set_size(&mut self, e: TableElement, size: u32) {
        self.sizes.retain(|(k, _)| *k != e);
        if e.has_size() && size != 1 {
            self.sizes.push((e, size.clamp(1, 9)));
        }
    }

    pub(crate) fn def(&self) -> TableStyleDef {
        let mut d = TableStyleDef::empty();
        for e in TableElement::ALL {
            d.set(e.xml(), self.format(e), self.size(e));
        }
        d
    }

    fn from_def(name: &str, d: &TableStyleDef) -> Self {
        let mut s = Self::new(name);
        for e in TableElement::ALL {
            if let Some((f, size)) = d.get(e.xml()) {
                s.set_format(e, f.clone());
                s.set_size(e, size);
            }
        }
        s
    }
}

// ---------------------------------------------------------------- state

/// The opened file's `<tableStyles>`, as it has them.
#[derive(Debug, Clone, Default)]
pub(crate) struct FileStyles {
    /// The whole element, verbatim.
    pub element: Option<String>,
    pub default_table: Option<String>,
    pub default_pivot: Option<String>,
    pub styles: Vec<FileStyle>,
}

/// One `<tableStyle>` of the file.
#[derive(Debug, Clone)]
pub(crate) struct FileStyle {
    pub name: String,
    /// Its element, verbatim.
    pub xml: String,
    /// For tables (`table` not `0`) and for pivot tables (`pivot` not `0`).
    pub table: bool,
    pub pivot: bool,
    pub def: TableStyleDef,
    pub elements: Vec<FileElement>,
}

/// One `<tableStyleElement>` of a file style.
#[derive(Debug, Clone)]
pub(crate) struct FileElement {
    pub id: Option<usize>,
    /// The `<dxf>` it points to, verbatim.
    pub dxf_xml: Option<String>,
}

/// The custom table styles of a history state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StyleSet {
    pub styles: Vec<StyleNow>,
    /// "Set as default table style for this document" (`None`: Excel's).
    pub default_table: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StyleNow {
    pub name: String,
    pub def: TableStyleDef,
    /// The file's style it is, unchanged (index in [`FileStyles::styles`]).
    pub file: Option<usize>,
    pub table: bool,
    pub pivot: bool,
}

impl StyleSet {
    fn from_file(f: &FileStyles) -> Self {
        Self {
            styles: f
                .styles
                .iter()
                .enumerate()
                .map(|(i, s)| StyleNow {
                    name: s.name.clone(),
                    def: s.def.clone(),
                    file: Some(i),
                    table: s.table,
                    pivot: s.pivot,
                })
                .collect(),
            default_table: f.default_table.clone(),
        }
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.styles.iter().position(|s| s.name == name).or_else(|| {
            self.styles
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(name))
        })
    }
}

/// What a save writes for table styles (see [`patch_styles`]).
#[derive(Debug, Clone)]
pub(crate) struct StylesSave {
    /// The file's element, when nothing changed: written as it is.
    pub verbatim: Option<String>,
    pub styles: Vec<StyleOut>,
    pub default_table: Option<String>,
    pub default_pivot: Option<String>,
    /// The model's dxf count: the saved styles part's dxfs beyond it are
    /// left from an earlier save of this session and dropped.
    pub model_dxfs: usize,
}

#[derive(Debug, Clone)]
pub(crate) enum StyleOut {
    /// A style of the file: its markup, and its dxfs by id.
    File {
        xml: String,
        dxfs: Vec<(usize, String)>,
    },
    /// A style made or changed in Kantor.
    Own {
        name: String,
        table: bool,
        pivot: bool,
        /// `(type, size, dxf)`.
        elements: Vec<(&'static str, Option<u32>, String)>,
    },
}

// ---------------------------------------------------------------- workbook

impl Workbook {
    /// The custom table styles now (the file's until one is edited).
    fn style_set(&self) -> std::borrow::Cow<'_, StyleSet> {
        match &self.edits().table_styles {
            Some(s) => std::borrow::Cow::Owned((**s).clone()),
            None => std::borrow::Cow::Owned(StyleSet::from_file(&self.table_styles.file)),
        }
    }

    /// A custom table style by name, for tables.
    pub(crate) fn custom_table_style(&self, name: &str) -> Option<TableStyleDef> {
        match &self.edits().table_styles {
            Some(set) => set
                .find(name)
                .map(|i| &set.styles[i])
                .filter(|s| s.table)
                .map(|s| s.def.clone()),
            None => self.table_styles.styles.get(name).cloned().or_else(|| {
                self.table_styles
                    .styles
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
            }),
        }
    }

    /// The workbook's own table styles (for tables), in the order the file
    /// and Kantor added them: the gallery's Custom group.
    pub fn custom_table_styles(&self) -> Vec<String> {
        self.style_set()
            .styles
            .iter()
            .filter(|s| s.table)
            .map(|s| s.name.clone())
            .collect()
    }

    /// The style new tables get: the document's default, else Excel's.
    pub fn default_table_style(&self) -> String {
        let set = self.style_set();
        set.default_table
            .clone()
            .filter(|d| set.find(d).is_some() || crate::table::is_builtin(d))
            .unwrap_or_else(|| DEFAULT_TABLE_STYLE.to_owned())
    }

    /// Style `name` (custom or built-in) as the New / Modify Table Style
    /// dialog shows it.
    pub fn table_style_spec(&self, name: &str) -> Option<TableStyleSpec> {
        let def = self.table_style(name)?;
        Some(TableStyleSpec::from_def(name, &def))
    }

    /// How `spec` draws a small table (5 columns: header row, five data
    /// rows, total row; first and last column, banded rows, and banded
    /// columns when a column stripe is formatted), as the New Table Style
    /// preview shows it.
    pub fn table_style_spec_preview(&self, spec: &TableStyleSpec) -> Vec<Vec<TableCellLook>> {
        let def = spec.def();
        let column_stripes = [
            TableElement::FirstColumnStripe,
            TableElement::SecondColumnStripe,
        ]
        .iter()
        .any(|e| !spec.format(*e).is_empty());
        let t = TableInfo {
            name: String::new(),
            area: CellRange::from_corners((0, 0), (6, 4)),
            header_rows: 1,
            totals_rows: 1,
            columns: vec![String::new(); 5],
            style: None,
            first_column: true,
            last_column: true,
            row_stripes: true,
            column_stripes,
            filter_buttons: false,
        };
        (0..7)
            .map(|r| (0..5).map(|c| def.look_pub(&t, r, c)).collect())
            .collect()
    }

    /// Whether `name` is taken by a table style (custom or built-in, any
    /// case), other than `except`.
    fn style_name_taken(&self, name: &str, except: Option<&str>) -> bool {
        if except.is_some_and(|e| e.eq_ignore_ascii_case(name)) {
            return false;
        }
        crate::table::is_builtin(name)
            || crate::pivot_tools::pivot_style_names()
                .iter()
                .any(|p| p.eq_ignore_ascii_case(name))
            || self
                .style_set()
                .styles
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// "Table Style 1", "Table Style 2"... the first one free.
    pub fn next_table_style_name(&self) -> String {
        (1..)
            .map(|i| format!("Table Style {i}"))
            .find(|n| !self.style_name_taken(n, None))
            .expect("a free name")
    }

    /// A free name for a copy of `name`: "Name 2", "Name 3"...
    pub fn duplicate_table_style_name(&self, name: &str) -> String {
        (2..)
            .map(|i| format!("{name} {i}"))
            .find(|n| !self.style_name_taken(n, None))
            .expect("a free name")
    }

    fn check_style_name(&self, name: &str, except: Option<&str>) -> Result<()> {
        let n = name.trim();
        if n.is_empty() {
            bail!("Type a name for the table style");
        }
        if n.chars().count() > 255 {
            bail!("A table style name has at most 255 characters");
        }
        if self.style_name_taken(n, except) {
            bail!("A table style with this name already exists");
        }
        Ok(())
    }

    /// New Table Style: add `spec` (one undo step); `default`: new tables
    /// in this document get it.
    pub fn add_table_style(&mut self, spec: &TableStyleSpec, default: bool) -> Result<()> {
        self.check_style_name(&spec.name, None)?;
        let mut set = self.style_set().into_owned();
        let name = spec.name.trim().to_owned();
        set.styles.push(StyleNow {
            name: name.clone(),
            def: spec.def(),
            file: None,
            table: true,
            pivot: false,
        });
        if default {
            set.default_table = Some(name);
        }
        self.set_style_set(set);
        Ok(())
    }

    /// Modify Table Style: style `name` becomes `spec` (renamed if its name
    /// differs: tables using it follow). One undo step.
    pub fn modify_table_style(
        &mut self,
        name: &str,
        spec: &TableStyleSpec,
        default: bool,
    ) -> Result<()> {
        let mut set = self.style_set().into_owned();
        let i = set
            .find(name)
            .ok_or_else(|| anyhow!("no table style {name}"))?;
        let old = set.styles[i].name.clone();
        self.check_style_name(&spec.name, Some(&old))?;
        let new = spec.name.trim().to_owned();
        let def = spec.def();
        let s = &mut set.styles[i];
        if s.def != def || s.name != new {
            s.file = None;
        }
        s.def = def;
        s.name = new.clone();
        if default {
            set.default_table = Some(new.clone());
        } else if set
            .default_table
            .as_deref()
            .is_some_and(|d| d.eq_ignore_ascii_case(&old))
        {
            set.default_table = None;
        }
        let renamed = old != new;
        self.batch(|wb| {
            if renamed {
                wb.restyle_tables(&old, &new)?;
            }
            wb.set_style_set(set);
            Ok(())
        })
    }

    /// Delete custom style `name` (one undo step): tables using it get the
    /// document's default style, as in Excel.
    pub fn delete_table_style(&mut self, name: &str) -> Result<()> {
        let mut set = self.style_set().into_owned();
        let i = set
            .find(name)
            .ok_or_else(|| anyhow!("no table style {name}"))?;
        let old = set.styles.remove(i).name;
        if set
            .default_table
            .as_deref()
            .is_some_and(|d| d.eq_ignore_ascii_case(&old))
        {
            set.default_table = None;
        }
        let fallback = set
            .default_table
            .clone()
            .unwrap_or_else(|| DEFAULT_TABLE_STYLE.to_owned());
        self.batch(|wb| {
            wb.restyle_tables(&old, &fallback)?;
            wb.set_style_set(set);
            Ok(())
        })
    }

    /// Set As Default for style `name` (custom or built-in). One undo step.
    pub fn set_default_table_style(&mut self, name: &str) -> Result<()> {
        if self.table_style(name).is_none() {
            bail!("no table style {name}");
        }
        let mut set = self.style_set().into_owned();
        set.default_table = Some(name.to_owned());
        self.set_style_set(set);
        Ok(())
    }

    /// Tables styled `old` get style `new`.
    fn restyle_tables(&mut self, old: &str, new: &str) -> Result<()> {
        let names: Vec<String> = self
            .model
            .get_model()
            .workbook
            .tables
            .values()
            .filter(|t| {
                t.style_info
                    .name
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(old))
            })
            .map(|t| t.name.clone())
            .collect();
        for n in names {
            let Some(mut o) = self.table_options(&n) else {
                continue;
            };
            o.style = Some(new.to_owned());
            self.set_table_options(&n, o)?;
        }
        Ok(())
    }

    fn set_style_set(&mut self, set: StyleSet) {
        self.own_edit(|e, _| e.table_styles = Some(Arc::new(set)));
    }

    /// What a save writes for table styles, if anything.
    pub(crate) fn table_style_patches(&self, out: &mut crate::preserve::Patches) {
        out.dxf_base = self.origin.dxf_base;
        let file = &self.table_styles.file;
        let edited = self.edits().table_styles.clone();
        let from_file = StyleSet::from_file(file);
        let set = edited.as_deref().unwrap_or(&from_file);
        if file.element.is_none() && set.styles.is_empty() && set.default_table.is_none() {
            return;
        }
        let model_dxfs = self.model.get_model().workbook.styles.dxfs.len();
        let verbatim = (*set == from_file).then(|| file.element.clone()).flatten();
        let styles = set
            .styles
            .iter()
            .map(|s| match s.file.and_then(|i| file.styles.get(i)) {
                Some(f) => StyleOut::File {
                    xml: f.xml.clone(),
                    dxfs: f
                        .elements
                        .iter()
                        .filter_map(|e| Some((e.id?, e.dxf_xml.clone()?)))
                        .collect(),
                },
                None => StyleOut::Own {
                    name: s.name.clone(),
                    table: s.table,
                    pivot: s.pivot,
                    elements: TableElement::ALL
                        .iter()
                        .filter_map(|e| {
                            let (f, size) = s.def.get(e.xml())?;
                            let size = (e.has_size() && size != 1).then_some(size);
                            (!f.is_empty() || size.is_some()).then(|| (e.xml(), size, dxf_xml(f)))
                        })
                        .collect(),
                },
            })
            .collect();
        out.table_styles = Some(StylesSave {
            verbatim,
            styles,
            default_table: set.default_table.clone(),
            default_pivot: file.default_pivot.clone(),
            model_dxfs,
        });
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn color(rgb: Rgb) -> String {
    format!("<color rgb=\"FF{rgb:06X}\"/>")
}

/// A `<dxf>` for element format `f`, in CT_Dxf's order.
pub(crate) fn dxf_xml(f: &Elem) -> String {
    let mut out = String::from("<dxf>");
    let flag = |name: &str, v: Option<bool>| match v {
        Some(true) => format!("<{name}/>"),
        Some(false) => format!("<{name} val=\"0\"/>"),
        None => String::new(),
    };
    let mut font = String::new();
    font.push_str(&flag("b", f.bold));
    font.push_str(&flag("i", f.italic));
    font.push_str(&flag("strike", f.strike));
    match f.underline {
        Some(true) => font.push_str("<u/>"),
        Some(false) => font.push_str("<u val=\"none\"/>"),
        None => {}
    }
    if let Some(c) = f.font {
        font.push_str(&color(c));
    }
    if !font.is_empty() {
        out.push_str(&format!("<font>{font}</font>"));
    }
    if let Some(c) = f.fill {
        out.push_str(&format!(
            "<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{c:06X}\"/><bgColor rgb=\"FF{c:06X}\"/></patternFill></fill>"
        ));
    }
    let edges: [(&str, Option<BorderLine>); 6] = [
        ("left", f.left),
        ("right", f.right),
        ("top", f.top),
        ("bottom", f.bottom),
        ("vertical", f.vertical),
        ("horizontal", f.horizontal),
    ];
    if edges.iter().any(|(_, l)| l.is_some()) {
        out.push_str("<border>");
        for (name, l) in edges {
            if let Some(l) = l {
                out.push_str(&format!("<{name} style=\"{}\">", l.style.xml()));
                out.push_str(&color(l.color.unwrap_or(0)));
                out.push_str(&format!("</{name}>"));
            }
        }
        out.push_str("</border>");
    }
    out.push_str("</dxf>");
    out
}

/// The saved styles part `xml` with the table styles of `save`: the dxfs
/// beyond the model's dropped, the file styles' dxfs as the file had them,
/// the dxfs of Kantor's styles appended, and `<tableStyles>` written after
/// `<dxfs>`.
pub(crate) fn patch_styles(xml: &str, save: &StylesSave) -> Result<String, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let main = root.tag_name().namespace();
    let is = |n: &roxmltree::Node, name: &str| {
        n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == main
    };
    let dxfs_node = root.children().find(|n| is(n, "dxfs"));
    let mut dxfs: Vec<String> = match dxfs_node {
        Some(d) => d
            .children()
            .filter(|n| is(n, "dxf"))
            .map(|n| crate::preserve::verbatim_pub(n, xml))
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    dxfs.truncate(save.model_dxfs);
    let table_styles = match &save.verbatim {
        Some(v) => {
            for s in &save.styles {
                if let StyleOut::File { dxfs: ids, .. } = s {
                    for (id, x) in ids {
                        if let Some(slot) = dxfs.get_mut(*id) {
                            *slot = x.clone();
                        }
                    }
                }
            }
            v.clone()
        }
        None => {
            let mut inner = String::new();
            for s in &save.styles {
                match s {
                    StyleOut::File { xml: sx, dxfs: ids } => {
                        for (id, x) in ids {
                            if let Some(slot) = dxfs.get_mut(*id) {
                                *slot = x.clone();
                            }
                        }
                        inner.push_str(sx);
                    }
                    StyleOut::Own {
                        name,
                        table,
                        pivot,
                        elements,
                    } => {
                        inner.push_str(&format!("<tableStyle name=\"{}\"", esc(name)));
                        if !pivot {
                            inner.push_str(" pivot=\"0\"");
                        }
                        if !table {
                            inner.push_str(" table=\"0\"");
                        }
                        inner.push_str(&format!(" count=\"{}\">", elements.len()));
                        for (kind, size, dx) in elements {
                            let id = dxfs.len();
                            dxfs.push(dx.clone());
                            inner.push_str(&format!(
                                "<tableStyleElement type=\"{kind}\"{} dxfId=\"{id}\"/>",
                                size.map(|s| format!(" size=\"{s}\"")).unwrap_or_default()
                            ));
                        }
                        inner.push_str("</tableStyle>");
                    }
                }
            }
            let mut out = format!(
                "<tableStyles count=\"{}\" defaultTableStyle=\"{}\" defaultPivotStyle=\"{}\"",
                save.styles.len(),
                esc(save.default_table.as_deref().unwrap_or(DEFAULT_TABLE_STYLE)),
                esc(save.default_pivot.as_deref().unwrap_or("PivotStyleLight16")),
            );
            if inner.is_empty() {
                out.push_str("/>");
            } else {
                out.push_str(&format!(">{inner}</tableStyles>"));
            }
            out
        }
    };
    let dxfs_text = if dxfs.is_empty() {
        "<dxfs count=\"0\"/>".to_owned()
    } else {
        format!("<dxfs count=\"{}\">{}</dxfs>", dxfs.len(), dxfs.concat())
    };
    // Edits, back to front: the old `<tableStyles>` out, `<dxfs>` replaced
    // (or added) with `<tableStyles>` after it.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let old_ts = root.children().find(|n| is(n, "tableStyles"));
    match dxfs_node {
        Some(d) => {
            if let Some(t) = old_ts {
                edits.push((t.range(), String::new()));
            }
            edits.push((d.range(), format!("{dxfs_text}{table_styles}")));
        }
        None => {
            let at = match old_ts {
                Some(t) => {
                    edits.push((t.range(), String::new()));
                    t.range().start
                }
                None => root
                    .children()
                    .find(|n| is(n, "colors") || is(n, "extLst"))
                    .map(|n| n.range().start)
                    .or_else(|| xml[..root.range().end].rfind("</"))
                    .ok_or("styles part is not closed")?,
            };
            edits.push((at..at, format!("{dxfs_text}{table_styles}")));
        }
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_owned();
    for (r, text) in edits {
        out.replace_range(r, &text);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::LineStyle;

    #[test]
    fn dxf_is_written_in_schema_order() {
        let f = Elem {
            bold: Some(true),
            font: Some(0xFFFFFF),
            fill: Some(0x7030A0),
            bottom: Some(BorderLine {
                style: LineStyle::Medium,
                color: Some(0x112233),
            }),
            ..Default::default()
        };
        assert_eq!(
            dxf_xml(&f),
            "<dxf><font><b/><color rgb=\"FFFFFFFF\"/></font><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF7030A0\"/><bgColor rgb=\"FF7030A0\"/></patternFill></fill><border><bottom style=\"medium\"><color rgb=\"FF112233\"/></bottom></border></dxf>"
        );
    }

    #[test]
    fn styles_part_gets_table_styles_after_dxfs() {
        let xml = "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><cellStyles count=\"0\"/><dxfs count=\"1\"><dxf><font><b/></font></dxf></dxfs></styleSheet>";
        let save = StylesSave {
            verbatim: None,
            styles: vec![StyleOut::Own {
                name: "Mine & yours".into(),
                table: true,
                pivot: false,
                elements: vec![("headerRow", None, "<dxf><font><i/></font></dxf>".into())],
            }],
            default_table: Some("Mine & yours".into()),
            default_pivot: None,
            model_dxfs: 1,
        };
        let out = patch_styles(xml, &save).unwrap();
        assert!(out.contains("<dxfs count=\"2\"><dxf><font><b/></font></dxf><dxf><font><i/></font></dxf></dxfs><tableStyles count=\"1\" defaultTableStyle=\"Mine &amp; yours\" defaultPivotStyle=\"PivotStyleLight16\"><tableStyle name=\"Mine &amp; yours\" pivot=\"0\" count=\"1\"><tableStyleElement type=\"headerRow\" dxfId=\"1\"/></tableStyle></tableStyles></styleSheet>"), "{out}");
        roxmltree::Document::parse(&out).unwrap();
    }
}
