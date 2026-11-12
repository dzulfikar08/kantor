//! Data validation, as Excel applies it: drop-down lists, input messages and
//! error alerts for typed values.
//!
//! IronCalc doesn't model validations. They are read from the opened file
//! (`<dataValidations>`, and the Excel 2010 form in `extLst` that Excel writes
//! when a list's source is on another sheet) and carried into saves verbatim
//! by `preserve`, which also moves their ranges and formulas with inserted
//! and deleted rows and columns. Here the same moves ([`crate::shift`]) map
//! each rule from the file to the current workbook state, so what the grid
//! shows agrees with what a save writes.
//!
//! Formulas (list sources, bounds, custom rules) are relative to the top-left
//! cell of the rule's first range, read stored values (a formula cell is
//! never recalculated to check a value) and are evaluated on demand, so a
//! list whose source depends on other cells follows them.
//!
//! Excel checks only what is typed into a cell: pasting, filling and
//! clearing don't validate. [`Workbook::invalid_cell`] tells which stored
//! values break their rule ("Circle Invalid Data").

use std::collections::HashMap;
use std::sync::Arc;

use ironcalc_base::{CfFormula, CfRef, CfValue};
use roxmltree::{Document, Node};

use crate::cond_format::{Value, cell_value, compare, general_number, parse_sqref, truthy};
use crate::shift::Layout;
use crate::{CellRange, Workbook};

/// The x14 data validation extension of `extLst`.
pub(crate) const DV_EXTENSION: &str = "{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}";
/// List items read at most (a whole-column source on a big sheet).
const MAX_LIST_ITEMS: usize = 10_000;
/// Cells whose check is kept at most (see [`Workbook::invalid_cell`]).
const MAX_CACHED_CELLS: usize = 100_000;

/// What a validation allows (Excel's "Allow").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationType {
    /// Any value: only an input message.
    Any,
    Whole,
    Decimal,
    List,
    Date,
    Time,
    TextLength,
    Custom,
}

/// How a value is compared with the bounds (`formula1`, `formula2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOperator {
    Between,
    NotBetween,
    Equal,
    NotEqual,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
}

/// The error alert's kind, which decides its buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorStyle {
    /// Retry / Cancel: the value can't stay.
    Stop,
    /// "Continue?" Yes / No / Cancel.
    Warning,
    /// OK / Cancel.
    Information,
}

/// A title and a text (input message or error alert); either may be empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    pub title: String,
    pub text: String,
}

/// One data validation rule, at its current place.
#[derive(Debug, Clone, PartialEq)]
pub struct Validation {
    pub kind: ValidationType,
    pub operator: ValidationOperator,
    /// Blank values pass ("Ignore blank").
    pub allow_blank: bool,
    /// A list shows its drop-down button. (The file's `showDropDown="1"`
    /// means the opposite: *hide* it.)
    pub dropdown: bool,
    /// Shown while a cell of the rule is selected (`showInputMessage`).
    pub prompt: Option<Message>,
    /// Shown when a typed value fails (`showErrorMessage`); without it a
    /// failing value is accepted silently, as in Excel.
    pub error: Option<(ErrorStyle, Message)>,
    /// `formula1` / `formula2` as stored (no `=`), moved with rows and
    /// columns. A list's `formula1` is `"a,b,c"` or a reference.
    pub formula1: Option<String>,
    pub formula2: Option<String>,
    /// The cells it applies to (`sqref`).
    pub ranges: Vec<CellRange>,
}

/// An item of a list validation's drop-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// As shown (a cell's formatted value).
    pub text: String,
    /// What picking it types into the cell.
    pub input: String,
}

/// A rule as read from the file, on sheet `sheet_id`.
#[derive(Debug, Clone)]
struct Raw {
    sheet_id: u32,
    sqref: String,
    rule: Validation,
}

/// The file's validations and the layout they were read in.
#[derive(Default)]
pub(crate) struct Validations {
    raw: Vec<Raw>,
    base: Layout,
    state: State,
}

/// Rules mapped to one workbook state, compiled on first use.
#[derive(Default)]
struct State {
    key: (u64, u64),
    sheets: HashMap<u32, Arc<Vec<Rule>>>,
    cells: HashMap<(u32, u32, u32), bool>,
}

struct Rule {
    v: Arc<Validation>,
    bounds: CellRange,
    f1: Option<CfFormula>,
    f2: Option<CfFormula>,
    /// An inline list (`"a,b,c"`).
    inline: Option<Vec<String>>,
}

impl Validations {
    /// Every validation of the package's worksheets.
    pub(crate) fn read(bytes: &[u8]) -> Self {
        let mut raw = Vec::new();
        match crate::preserve::worksheet_skeletons(bytes) {
            Ok(sheets) => {
                for (sheet_id, xml) in sheets {
                    if xml.contains("dataValidation") {
                        raw.extend(parse_sheet(sheet_id, &xml));
                    }
                }
            }
            Err(e) => log::warn!("data validation not read: {e}"),
        }
        Validations {
            raw,
            base: Layout::default(),
            state: State::default(),
        }
    }
}

fn flag(n: Node, name: &str) -> bool {
    matches!(n.attribute(name), Some("1" | "true"))
}

fn text_attr(n: Node, name: &str) -> String {
    n.attribute(name).unwrap_or_default().to_owned()
}

/// The `<dataValidation>` elements of a worksheet (skeleton) XML, plain and
/// x14.
fn parse_sheet(sheet_id: u32, xml: &str) -> Vec<Raw> {
    let doc = match Document::parse(xml) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("data validation of sheet {sheet_id} not read: {e}");
            return Vec::new();
        }
    };
    let root = doc.root_element();
    let mut out = Vec::new();
    for child in root.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "dataValidations" => {
                for dv in child
                    .children()
                    .filter(|n| n.tag_name().name() == "dataValidation")
                {
                    let f = |name: &str| {
                        dv.children()
                            .find(|c| c.tag_name().name() == name)
                            .map(|c| c.text().unwrap_or_default().to_owned())
                    };
                    let sqref = text_attr(dv, "sqref");
                    if let Some(raw) = parse_rule(sheet_id, dv, sqref, f("formula1"), f("formula2"))
                    {
                        out.push(raw);
                    }
                }
            }
            "extLst" => {
                for ext in child.children().filter(|n| {
                    n.tag_name().name() == "ext" && n.attribute("uri") == Some(DV_EXTENSION)
                }) {
                    for dv in ext
                        .descendants()
                        .filter(|n| n.tag_name().name() == "dataValidation")
                    {
                        // <x14:formula1><xm:f>...</xm:f></x14:formula1>
                        let f = |name: &str| {
                            dv.children()
                                .find(|c| c.tag_name().name() == name)
                                .and_then(|c| c.children().find(|c| c.tag_name().name() == "f"))
                                .map(|c| c.text().unwrap_or_default().to_owned())
                        };
                        let sqref = dv
                            .children()
                            .find(|c| c.tag_name().name() == "sqref")
                            .and_then(|c| c.text())
                            .unwrap_or_default()
                            .to_owned();
                        if let Some(raw) =
                            parse_rule(sheet_id, dv, sqref, f("formula1"), f("formula2"))
                        {
                            out.push(raw);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn parse_rule(
    sheet_id: u32,
    dv: Node,
    sqref: String,
    formula1: Option<String>,
    formula2: Option<String>,
) -> Option<Raw> {
    let kind = match dv.attribute("type").unwrap_or("none") {
        "none" => ValidationType::Any,
        "whole" => ValidationType::Whole,
        "decimal" => ValidationType::Decimal,
        "list" => ValidationType::List,
        "date" => ValidationType::Date,
        "time" => ValidationType::Time,
        "textLength" => ValidationType::TextLength,
        "custom" => ValidationType::Custom,
        _ => return None,
    };
    let operator = match dv.attribute("operator").unwrap_or("between") {
        "notBetween" => ValidationOperator::NotBetween,
        "equal" => ValidationOperator::Equal,
        "notEqual" => ValidationOperator::NotEqual,
        "greaterThan" => ValidationOperator::GreaterThan,
        "lessThan" => ValidationOperator::LessThan,
        "greaterThanOrEqual" => ValidationOperator::GreaterThanOrEqual,
        "lessThanOrEqual" => ValidationOperator::LessThanOrEqual,
        _ => ValidationOperator::Between,
    };
    let prompt = Message {
        title: text_attr(dv, "promptTitle"),
        text: text_attr(dv, "prompt"),
    };
    let error = Message {
        title: text_attr(dv, "errorTitle"),
        text: text_attr(dv, "error"),
    };
    let style = match dv.attribute("errorStyle") {
        Some("warning") => ErrorStyle::Warning,
        Some("information") => ErrorStyle::Information,
        _ => ErrorStyle::Stop,
    };
    let empty = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    let rule = Validation {
        kind,
        operator,
        allow_blank: flag(dv, "allowBlank"),
        dropdown: !flag(dv, "showDropDown"),
        prompt: (flag(dv, "showInputMessage")
            && !(prompt.title.is_empty() && prompt.text.is_empty()))
        .then_some(prompt),
        error: flag(dv, "showErrorMessage").then_some((style, error)),
        formula1: (!empty(&formula1)).then(|| formula1.unwrap_or_default()),
        formula2: (!empty(&formula2)).then(|| formula2.unwrap_or_default()),
        ranges: Vec::new(),
    };
    (!sqref.trim().is_empty()).then_some(Raw {
        sheet_id,
        sqref,
        rule,
    })
}

/// The items of an inline list (`"a,b,c"`), or `None` for a reference.
fn inline_items(formula: &str) -> Option<Vec<String>> {
    let f = formula.trim();
    let inner = f.strip_prefix('"')?.strip_suffix('"')?;
    let inner = inner.replace("\"\"", "\"");
    Some(
        inner
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Empty | Value::Error => String::new(),
        Value::Number(n) => general_number(*n),
        Value::Text(s) => s.clone(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
    }
}

/// A bound as a number: blanks are 0, text that reads as a number counts.
fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Empty => Some(0.0),
        Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn compare_op(op: ValidationOperator, x: f64, a: f64, b: Option<f64>) -> bool {
    use ValidationOperator::*;
    // Bounds given the wrong way round still mean the range between them.
    let between = |b: f64| x >= a.min(b) && x <= a.max(b);
    match op {
        Between => b.is_some_and(between),
        NotBetween => b.is_some_and(|b| !between(b)),
        Equal => x == a,
        NotEqual => x != a,
        GreaterThan => x > a,
        LessThan => x < a,
        GreaterThanOrEqual => x >= a,
        LessThanOrEqual => x <= a,
    }
}

impl Workbook {
    /// Whether `sheet` has data validation.
    pub fn has_validations(&mut self, sheet: u32) -> bool {
        !self.validation_rules(sheet).is_empty()
    }

    /// The validation of a cell (0-based), if any. When rules overlap (which
    /// Excel doesn't create) the first one wins.
    pub fn validation(&mut self, sheet: u32, row: u32, col: u32) -> Option<Arc<Validation>> {
        let rules = self.validation_rules(sheet);
        rules
            .iter()
            .find(|r| {
                r.bounds.contains(row, col) && r.v.ranges.iter().any(|a| a.contains(row, col))
            })
            .map(|r| r.v.clone())
    }

    /// The drop-down items of a cell's list validation, evaluated now (a
    /// source that depends on other cells follows them); `None` if the cell
    /// has no list validation. Blank cells of a source range are left out.
    pub fn validation_list(&mut self, sheet: u32, row: u32, col: u32) -> Option<Vec<ListItem>> {
        let rules = self.validation_rules(sheet);
        let rule = rules.iter().find(|r| {
            r.v.kind == ValidationType::List && r.v.ranges.iter().any(|a| a.contains(row, col))
        })?;
        let values = self.list_values(rule, sheet, row, col);
        Some(
            values
                .into_iter()
                .filter(|(v, text)| !matches!(v, Value::Empty) && !text.is_empty())
                .map(|(v, text)| {
                    let input = match &v {
                        // Text that would be read as something else
                        // (a number, a formula) stays text.
                        Value::Text(s)
                            if self.model.kantor_input_value(s)
                                != Some(CfValue::Text(s.clone())) =>
                        {
                            format!("'{s}")
                        }
                        Value::Text(s) => s.clone(),
                        _ => text.clone(),
                    };
                    ListItem { text, input }
                })
                .collect(),
        )
    }

    /// The validation a cell's stored value breaks, if it breaks one. Blank
    /// cells and formulas waiting for a recalculation pass.
    pub fn validate_cell(&mut self, sheet: u32, row: u32, col: u32) -> Option<Arc<Validation>> {
        let rules = self.validation_rules(sheet);
        let rule = rules.iter().find(|r| {
            r.bounds.contains(row, col) && r.v.ranges.iter().any(|a| a.contains(row, col))
        })?;
        if self.is_pending(sheet, row, col) {
            return None;
        }
        let value = cell_value(
            self.model.get_model(),
            sheet,
            row as i32 + 1,
            col as i32 + 1,
        );
        (!self.passes(rule, sheet, row, col, &value)).then(|| rule.v.clone())
    }

    /// Whether a cell's stored value breaks its validation, worked out once
    /// per workbook state (for circling invalid data). Blank cells are never
    /// circled.
    pub fn invalid_cell(&mut self, sheet: u32, row: u32, col: u32) -> bool {
        if self.validation_rules(sheet).is_empty() {
            return false;
        }
        if let Some(&hit) = self.validation.state.cells.get(&(sheet, row, col)) {
            return hit;
        }
        let blank = matches!(
            cell_value(
                self.model.get_model(),
                sheet,
                row as i32 + 1,
                col as i32 + 1
            ),
            Value::Empty
        );
        let bad = !blank && self.validate_cell(sheet, row, col).is_some();
        let cells = &mut self.validation.state.cells;
        if cells.len() >= MAX_CACHED_CELLS {
            cells.clear();
        }
        cells.insert((sheet, row, col), bad);
        bad
    }

    /// Take back the last edit for good (undo it and forget its redo): a
    /// typed value its validation rejected.
    pub fn revert_edit(&mut self) -> anyhow::Result<()> {
        if self.undo()? {
            self.model.kantor_forget_redo();
            self.history.redo.clear();
        }
        Ok(())
    }

    fn passes(&mut self, rule: &Rule, sheet: u32, row: u32, col: u32, value: &Value) -> bool {
        let v = &rule.v;
        if matches!(value, Value::Empty) && v.allow_blank {
            return true;
        }
        let (r, c) = (row as i32 + 1, col as i32 + 1);
        let eval = |f: &Option<CfFormula>, wb: &mut Workbook| -> Option<Value> {
            f.as_ref()
                .map(|f| Value::from(wb.model.cf_evaluate(f, sheet, r, c)))
        };
        match v.kind {
            ValidationType::Any => true,
            ValidationType::Whole
            | ValidationType::Decimal
            | ValidationType::Date
            | ValidationType::Time => {
                let Value::Number(x) = value else {
                    return false;
                };
                if v.kind == ValidationType::Whole && x.fract() != 0.0 {
                    return false;
                }
                let Some(a) = eval(&rule.f1, self).as_ref().and_then(as_number) else {
                    return false;
                };
                let b = eval(&rule.f2, self).as_ref().and_then(as_number);
                compare_op(v.operator, *x, a, b)
            }
            ValidationType::TextLength => {
                if matches!(value, Value::Error) {
                    return false;
                }
                let len = value_text(value).chars().count() as f64;
                let Some(a) = eval(&rule.f1, self).as_ref().and_then(as_number) else {
                    return false;
                };
                let b = eval(&rule.f2, self).as_ref().and_then(as_number);
                compare_op(v.operator, len, a, b)
            }
            ValidationType::Custom => eval(&rule.f1, self).is_some_and(|r| truthy(&r)),
            ValidationType::List => {
                if matches!(value, Value::Error) {
                    return false;
                }
                self.list_values(rule, sheet, row, col)
                    .iter()
                    .any(|(item, _)| {
                        !matches!(item, Value::Empty)
                            && compare(item, value) == Some(std::cmp::Ordering::Equal)
                            && std::mem::discriminant(item) == std::mem::discriminant(value)
                    })
            }
        }
    }

    /// A list rule's values with their displayed text, for cell (`row`,
    /// `col`) (a relative source moves with the cell).
    fn list_values(&mut self, rule: &Rule, sheet: u32, row: u32, col: u32) -> Vec<(Value, String)> {
        if let Some(items) = &rule.inline {
            return items
                .iter()
                .map(|s| {
                    let v = match self.model.kantor_input_value(s) {
                        Some(CfValue::Number(n)) => Value::Number(n),
                        Some(CfValue::Bool(b)) => Value::Bool(b),
                        _ => Value::Text(s.clone()),
                    };
                    (v, s.clone())
                })
                .collect();
        }
        let Some(f) = &rule.f1 else {
            return Vec::new();
        };
        match self
            .model
            .cf_evaluate_ref(f, sheet, row as i32 + 1, col as i32 + 1)
        {
            CfRef::Range {
                sheet: s,
                top,
                left,
                bottom,
                right,
            } => {
                // Whole columns or rows: only the used part.
                let (used_r, used_c) = self.used_size(s);
                let bottom = bottom.min(used_r as i32);
                let right = right.min(used_c as i32);
                let mut out = Vec::new();
                'rows: for r in top..=bottom {
                    for c in left..=right {
                        if out.len() >= MAX_LIST_ITEMS {
                            break 'rows;
                        }
                        let v = cell_value(self.model.get_model(), s, r, c);
                        let text = self.display_value(s, r as u32 - 1, c as u32 - 1);
                        out.push((v, text));
                    }
                }
                out
            }
            CfRef::Array(rows) => rows
                .into_iter()
                .flatten()
                .take(MAX_LIST_ITEMS)
                .map(|v| {
                    let v = Value::from(v);
                    let text = value_text(&v);
                    (v, text)
                })
                .collect(),
            CfRef::Value(v) => {
                // A single value: a text is a comma-separated list.
                let v = Value::from(v);
                match v {
                    Value::Text(s) => s
                        .split(',')
                        .map(|p| (Value::Text(p.trim().to_owned()), p.trim().to_owned()))
                        .collect(),
                    v => {
                        let text = value_text(&v);
                        vec![(v, text)]
                    }
                }
            }
        }
    }

    /// The rules of `sheet` (an index) in the current state.
    fn validation_rules(&mut self, sheet: u32) -> Arc<Vec<Rule>> {
        if self.validation.raw.is_empty() && self.edits().validations.is_empty() {
            return Arc::default();
        }
        let key = self.revision();
        if self.validation.state.key != key {
            self.validation.state = State {
                key,
                ..State::default()
            };
        }
        if let Some(r) = self.validation.state.sheets.get(&sheet) {
            return r.clone();
        }
        let rules = Arc::new(self.compile_validations(sheet));
        self.validation.state.sheets.insert(sheet, rules.clone());
        rules
    }

    /// The rules of `sheet` moved to the current state (no formulas
    /// parsed): Kantor's edits for the sheet if any, else the file's.
    pub(crate) fn moved_validations(&self, sheet: u32) -> Vec<Validation> {
        let sheet_id = self.sheet_id(sheet);
        // Rules edited in Kantor replace the file's for the whole sheet.
        let edits = self.edits();
        let (raws, base): (Vec<Raw>, Layout) = match edits.validations.get(&sheet_id) {
            Some((list, layout)) => (
                list.iter()
                    .map(|v| Raw {
                        sheet_id,
                        sqref: crate::sqref_of(&v.ranges),
                        rule: v.clone(),
                    })
                    .collect(),
                layout.clone(),
            ),
            None => (
                self.validation
                    .raw
                    .iter()
                    .filter(|r| r.sheet_id == sheet_id)
                    .cloned()
                    .collect(),
                self.validation.base.clone(),
            ),
        };
        if raws.is_empty() {
            return Vec::new();
        }
        let moves = base.moves_to(&self.history.layout());
        let names: HashMap<String, u32> = self
            .model
            .get_model()
            .workbook
            .worksheets
            .iter()
            .map(|w| (w.get_name().to_lowercase(), w.sheet_id))
            .collect();
        let mut out = Vec::new();
        for raw in raws {
            let Some(sqref) = moves.sqref(sheet_id, &raw.sqref) else {
                continue; // every cell deleted
            };
            let ranges = parse_sqref(&sqref);
            if ranges.is_empty() {
                continue;
            }
            let mut v = raw.rule;
            let moved = |f: &Option<String>| {
                f.as_ref()
                    .map(|f| moves.formula(f, Some(sheet_id), &names).into_owned())
            };
            v.formula1 = moved(&v.formula1);
            v.formula2 = moved(&v.formula2);
            v.ranges = ranges;
            out.push(v);
        }
        out
    }

    fn compile_validations(&mut self, sheet: u32) -> Vec<Rule> {
        let mut out = Vec::new();
        for v in self.moved_validations(sheet) {
            let first = v.ranges[0];
            let bounds = v.ranges.iter().skip(1).fold(first, |b, r| CellRange {
                top: b.top.min(r.top),
                left: b.left.min(r.left),
                bottom: b.bottom.max(r.bottom),
                right: b.right.max(r.right),
            });
            let anchor = (first.top as i32 + 1, first.left as i32 + 1);
            let inline = (v.kind == ValidationType::List)
                .then(|| v.formula1.as_deref().and_then(inline_items))
                .flatten();
            let mut parse = |f: &Option<String>| {
                f.as_ref()
                    .and_then(|f| self.model.cf_parse(sheet, f, anchor.0, anchor.1))
            };
            let f1 = if inline.is_some() {
                None
            } else {
                parse(&v.formula1)
            };
            let f2 = parse(&v.formula2);
            out.push(Rule {
                v: Arc::new(v),
                bounds,
                f1,
                f2,
                inline,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_lists() {
        assert_eq!(
            inline_items("\"Yes,No, Maybe\""),
            Some(vec!["Yes".into(), "No".into(), "Maybe".into()])
        );
        assert_eq!(inline_items("$A$1:$A$3"), None);
        assert_eq!(
            inline_items("\"say \"\"hi\"\",bye\""),
            Some(vec!["say \"hi\"".into(), "bye".into()])
        );
    }

    #[test]
    fn operators() {
        use ValidationOperator::*;
        assert!(compare_op(Between, 5.0, 1.0, Some(10.0)));
        assert!(compare_op(Between, 5.0, 10.0, Some(1.0)));
        assert!(!compare_op(Between, 11.0, 1.0, Some(10.0)));
        assert!(compare_op(NotBetween, 11.0, 1.0, Some(10.0)));
        assert!(!compare_op(NotBetween, 1.0, 1.0, Some(10.0)));
        assert!(compare_op(Equal, 3.0, 3.0, None));
        assert!(compare_op(NotEqual, 2.0, 3.0, None));
        assert!(compare_op(GreaterThan, 4.0, 3.0, None));
        assert!(!compare_op(GreaterThan, 3.0, 3.0, None));
        assert!(compare_op(LessThan, 2.0, 3.0, None));
        assert!(compare_op(GreaterThanOrEqual, 3.0, 3.0, None));
        assert!(compare_op(LessThanOrEqual, 3.0, 3.0, None));
        assert!(
            !compare_op(Between, 3.0, 1.0, None),
            "between needs two bounds"
        );
    }

    #[test]
    fn parses_plain_and_x14_rules() {
        let xml = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><sheetData/>
<dataValidations count="2">
<dataValidation type="whole" operator="greaterThan" allowBlank="1" showInputMessage="1" showErrorMessage="1" errorStyle="warning" errorTitle="Too small" error="Over 5" promptTitle="Qty" prompt="A number" sqref="B2:B5 D1"><formula1>5</formula1></dataValidation>
<dataValidation type="list" showDropDown="1" sqref="C1"><formula1>"a,b"</formula1></dataValidation>
</dataValidations>
<extLst><ext uri="{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}"><x14:dataValidations count="1"><x14:dataValidation type="list" allowBlank="1"><x14:formula1><xm:f>Lists!$A$1:$A$3</xm:f></x14:formula1><xm:sqref>E1:E9</xm:sqref></x14:dataValidation></x14:dataValidations></ext></extLst></worksheet>"#;
        let rules = parse_sheet(1, xml);
        assert_eq!(rules.len(), 3);
        let r = &rules[0].rule;
        assert_eq!(rules[0].sqref, "B2:B5 D1");
        assert_eq!(r.kind, ValidationType::Whole);
        assert_eq!(r.operator, ValidationOperator::GreaterThan);
        assert!(r.allow_blank);
        assert_eq!(r.formula1.as_deref(), Some("5"));
        assert_eq!(
            r.prompt,
            Some(Message {
                title: "Qty".into(),
                text: "A number".into()
            })
        );
        assert_eq!(
            r.error,
            Some((
                ErrorStyle::Warning,
                Message {
                    title: "Too small".into(),
                    text: "Over 5".into()
                }
            ))
        );
        // showDropDown="1" hides the button; no showErrorMessage, no alert.
        assert!(!rules[1].rule.dropdown);
        assert!(rules[1].rule.error.is_none());
        assert_eq!(rules[2].sqref, "E1:E9");
        assert_eq!(rules[2].rule.formula1.as_deref(), Some("Lists!$A$1:$A$3"));
        assert!(rules[2].rule.dropdown);
    }
}
