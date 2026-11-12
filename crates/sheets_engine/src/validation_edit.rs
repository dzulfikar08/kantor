//! Data Validation… in Kantor: set a rule on cells (Excel's "apply to the
//! selection": the cells leave any rule they had) or clear them (Clear All).
//!
//! Once a sheet's validations are edited, its whole list lives in the
//! history state's [`crate::edits::Edits`] (with the layout it was made in,
//! so ranges and formulas follow later row and column moves); a save writes
//! that list as `<dataValidations>` and drops the file's Excel 2010 form of
//! them. Sheets nobody edited keep the file's markup byte for byte.

use anyhow::{Result, bail};

use crate::validation::{Validation, ValidationOperator, ValidationType};
use crate::{CellRange, ErrorStyle, Workbook};

/// `a` without the cells of `b`: up to four rectangles.
pub(crate) fn subtract(a: CellRange, b: CellRange) -> Vec<CellRange> {
    if !a.intersects(&b) {
        return vec![a];
    }
    let mut out = Vec::new();
    if a.top < b.top {
        out.push(CellRange {
            bottom: b.top - 1,
            ..a
        });
    }
    if a.bottom > b.bottom {
        out.push(CellRange {
            top: b.bottom + 1,
            ..a
        });
    }
    let top = a.top.max(b.top);
    let bottom = a.bottom.min(b.bottom);
    if a.left < b.left {
        out.push(CellRange {
            top,
            bottom,
            left: a.left,
            right: b.left - 1,
        });
    }
    if a.right > b.right {
        out.push(CellRange {
            top,
            bottom,
            left: b.right + 1,
            right: a.right,
        });
    }
    out
}

/// Join rectangles that line up (same columns and touching rows, or same
/// rows and touching columns) so cut ranges don't stay in pieces.
pub(crate) fn coalesce(mut v: Vec<CellRange>) -> Vec<CellRange> {
    loop {
        let mut joined = false;
        'outer: for i in 0..v.len() {
            for j in 0..v.len() {
                if i == j {
                    continue;
                }
                let (a, b) = (v[i], v[j]);
                let vertical = a.left == b.left && a.right == b.right && a.bottom + 1 == b.top;
                let horizontal = a.top == b.top && a.bottom == b.bottom && a.right + 1 == b.left;
                if vertical || horizontal {
                    v[i] = CellRange::from_corners((a.top, a.left), (b.bottom, b.right));
                    v.remove(j);
                    joined = true;
                    break 'outer;
                }
            }
        }
        if !joined {
            break;
        }
    }
    v.sort_by_key(|r| (r.top, r.left));
    v
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\n', "&#10;")
}

/// The `<dataValidations>` element for `rules` (`None` when there are none).
pub(crate) fn validations_xml(rules: &[Validation]) -> Option<String> {
    let rules: Vec<&Validation> = rules.iter().filter(|r| !r.ranges.is_empty()).collect();
    if rules.is_empty() {
        return None;
    }
    let mut x = format!("<dataValidations count=\"{}\">", rules.len());
    for r in rules {
        x.push_str("<dataValidation");
        let ty = match r.kind {
            ValidationType::Any => None,
            ValidationType::Whole => Some("whole"),
            ValidationType::Decimal => Some("decimal"),
            ValidationType::List => Some("list"),
            ValidationType::Date => Some("date"),
            ValidationType::Time => Some("time"),
            ValidationType::TextLength => Some("textLength"),
            ValidationType::Custom => Some("custom"),
        };
        if let Some(t) = ty {
            x.push_str(&format!(" type=\"{t}\""));
        }
        if let Some((style, _)) = &r.error {
            match style {
                ErrorStyle::Stop => {}
                ErrorStyle::Warning => x.push_str(" errorStyle=\"warning\""),
                ErrorStyle::Information => x.push_str(" errorStyle=\"information\""),
            }
        }
        let compares = matches!(
            r.kind,
            ValidationType::Whole
                | ValidationType::Decimal
                | ValidationType::Date
                | ValidationType::Time
                | ValidationType::TextLength
        );
        if compares {
            let op = match r.operator {
                ValidationOperator::Between => None,
                ValidationOperator::NotBetween => Some("notBetween"),
                ValidationOperator::Equal => Some("equal"),
                ValidationOperator::NotEqual => Some("notEqual"),
                ValidationOperator::GreaterThan => Some("greaterThan"),
                ValidationOperator::LessThan => Some("lessThan"),
                ValidationOperator::GreaterThanOrEqual => Some("greaterThanOrEqual"),
                ValidationOperator::LessThanOrEqual => Some("lessThanOrEqual"),
            };
            if let Some(op) = op {
                x.push_str(&format!(" operator=\"{op}\""));
            }
        }
        if r.allow_blank {
            x.push_str(" allowBlank=\"1\"");
        }
        if r.kind == ValidationType::List && !r.dropdown {
            // The attribute's name says the opposite: 1 hides the button.
            x.push_str(" showDropDown=\"1\"");
        }
        if r.prompt.is_some() {
            x.push_str(" showInputMessage=\"1\"");
        }
        if r.error.is_some() {
            x.push_str(" showErrorMessage=\"1\"");
        }
        if let Some((_, m)) = &r.error {
            if !m.title.is_empty() {
                x.push_str(&format!(" errorTitle=\"{}\"", esc(&m.title)));
            }
            if !m.text.is_empty() {
                x.push_str(&format!(" error=\"{}\"", esc(&m.text)));
            }
        }
        if let Some(m) = &r.prompt {
            if !m.title.is_empty() {
                x.push_str(&format!(" promptTitle=\"{}\"", esc(&m.title)));
            }
            if !m.text.is_empty() {
                x.push_str(&format!(" prompt=\"{}\"", esc(&m.text)));
            }
        }
        x.push_str(&format!(" sqref=\"{}\">", crate::sqref_of(&r.ranges)));
        if let Some(f) = &r.formula1 {
            x.push_str(&format!("<formula1>{}</formula1>", esc(f)));
        }
        let two = matches!(
            r.operator,
            ValidationOperator::Between | ValidationOperator::NotBetween
        );
        if compares
            && two
            && let Some(f) = &r.formula2
        {
            x.push_str(&format!("<formula2>{}</formula2>", esc(f)));
        }
        x.push_str("</dataValidation>");
    }
    x.push_str("</dataValidations>");
    Some(x)
}

/// What a typed bound becomes in `formula1`/`formula2` for `kind`: a
/// number, a date (`2026-01-31`, `31/01/2026`) or time (`9:30`) as Excel's
/// serial number, `=formula` without its `=`.
pub fn validation_operand(kind: ValidationType, text: &str) -> Result<String> {
    let t = text.trim();
    if t.is_empty() {
        bail!("Type a value");
    }
    if let Some(f) = t.strip_prefix('=') {
        return Ok(f.trim().to_owned());
    }
    let comma = t.replace(',', ".");
    match kind {
        ValidationType::Date => {
            if let Some(n) = date_serial(t) {
                return Ok(n.to_string());
            }
            if comma.parse::<f64>().is_ok() {
                return Ok(comma);
            }
            bail!("Type a date such as 2026-01-31 or 31/01/2026")
        }
        ValidationType::Time => {
            if let Some(f) = time_fraction(t) {
                return Ok(trim_number(f));
            }
            bail!("Type a time such as 9:30")
        }
        ValidationType::Whole | ValidationType::TextLength => match comma.parse::<f64>() {
            Ok(v) if v.fract() == 0.0 => Ok(trim_number(v)),
            _ => bail!("Type a whole number"),
        },
        _ => match comma.parse::<f64>() {
            Ok(v) => Ok(trim_number(v)),
            Err(_) => bail!("Type a number"),
        },
    }
}

/// A stored bound as Data Validation shows it: dates and times readable,
/// formulas with `=`.
pub fn validation_operand_text(kind: ValidationType, formula: &str) -> String {
    let f = formula.trim();
    match f.parse::<f64>() {
        Ok(n) if kind == ValidationType::Date => serial_date(n).unwrap_or_else(|| f.to_owned()),
        Ok(n) if kind == ValidationType::Time => {
            let mins = (n.fract() * 24.0 * 60.0).round() as i64;
            format!("{}:{:02}", mins / 60, mins % 60)
        }
        Ok(_) => f.to_owned(),
        Err(_) => format!("={f}"),
    }
}

fn trim_number(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('e') {
        format!("{v:.10}")
    } else {
        s
    }
}

/// Days from 1899-12-30 (Excel's serial, 1900 system).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_serial(t: &str) -> Option<i64> {
    let parts: Vec<&str> = t.split(['-', '/', '.']).collect();
    if parts.len() != 3 {
        return None;
    }
    let n: Vec<i64> = parts
        .iter()
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let (y, m, d) = if parts[0].len() == 4 {
        (n[0], n[1], n[2])
    } else {
        // Day first, as in Indonesia (and most of the world).
        (n[2], n[1], n[0])
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=9999).contains(&y) {
        return None;
    }
    Some(days_from_civil(y, m, d) - days_from_civil(1899, 12, 30))
}

fn serial_date(n: f64) -> Option<String> {
    if !(1.0..2_958_466.0).contains(&n) || n.fract() != 0.0 {
        return None;
    }
    let z = n as i64 + days_from_civil(1899, 12, 30) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn time_fraction(t: &str) -> Option<f64> {
    let parts: Vec<&str> = t.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let h: f64 = parts[0].trim().parse().ok()?;
    let m: f64 = parts[1].trim().parse().ok()?;
    let s: f64 = parts.get(2).map_or(Some(0.0), |p| p.trim().parse().ok())?;
    if !(0.0..24.0).contains(&h) || !(0.0..60.0).contains(&m) || !(0.0..60.0).contains(&s) {
        return None;
    }
    Some((h * 3600.0 + m * 60.0 + s) / 86_400.0)
}

/// A list source as typed: `a, b, c` (items) or `=$A$1:$A$9` (cells).
pub fn list_source(text: &str) -> Result<String> {
    let t = text.trim();
    if let Some(f) = t.strip_prefix('=') {
        if f.trim().is_empty() {
            bail!("Type the cells of the list, such as =$A$1:$A$9");
        }
        return Ok(f.trim().to_owned());
    }
    let items: Vec<&str> = t
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if items.is_empty() {
        bail!("Type the list's items separated by commas, or =cells");
    }
    let joined = items.join(",");
    if joined.len() > 255 {
        bail!("A typed list holds at most 255 characters: put the items in cells");
    }
    Ok(format!("\"{}\"", joined.replace('"', "\"\"")))
}

/// The other way: `"a,b,c"` → `a, b, c`, a reference with `=`.
pub fn list_source_text(formula: &str) -> String {
    let f = formula.trim();
    match f.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(inner) => inner
            .replace("\"\"", "\"")
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(", "),
        None => format!("={f}"),
    }
}

impl Workbook {
    /// The validations of `sheet` where they are now.
    pub fn sheet_validations(&self, sheet: u32) -> Vec<Validation> {
        self.moved_validations(sheet)
    }

    /// Put `rule` on `ranges` of `sheet` (Data Validation… OK): the cells
    /// leave any rule they had. One undo step.
    pub fn set_validation(
        &mut self,
        sheet: u32,
        ranges: &[CellRange],
        rule: Validation,
    ) -> Result<()> {
        if ranges.is_empty() {
            bail!("Select the cells to validate");
        }
        if rule.kind == ValidationType::List && rule.formula1.is_none() {
            bail!("Type the list's source");
        }
        let mut list = self.without_cells(sheet, ranges);
        let mut rule = rule;
        rule.ranges = ranges.to_vec();
        list.push(rule);
        self.put_validations(sheet, list);
        Ok(())
    }

    /// Clear All: no validation on `ranges` of `sheet`. One undo step.
    pub fn clear_validation(&mut self, sheet: u32, ranges: &[CellRange]) -> Result<()> {
        let list = self.without_cells(sheet, ranges);
        self.put_validations(sheet, list);
        Ok(())
    }

    /// The sheet's rules with `ranges` cut out of them (rules left with no
    /// cells go).
    fn without_cells(&self, sheet: u32, ranges: &[CellRange]) -> Vec<Validation> {
        let mut list = self.sheet_validations(sheet);
        for v in &mut list {
            let mut cells = v.ranges.clone();
            for cut in ranges {
                cells = cells.into_iter().flat_map(|r| subtract(r, *cut)).collect();
            }
            v.ranges = coalesce(cells);
        }
        list.retain(|v| !v.ranges.is_empty());
        list
    }

    fn put_validations(&mut self, sheet: u32, list: Vec<Validation>) {
        let id = self.sheet_id(sheet);
        self.own_edit(|e, layout| {
            e.validations.insert(id, (list, layout.clone()));
        });
    }

    /// Patches a save writes for validations edited in Kantor.
    pub(crate) fn validation_patches(&self, out: &mut crate::preserve::Patches) {
        let ids: Vec<u32> = self.edits().validations.keys().copied().collect();
        for id in ids {
            let Some(sheet) =
                (0..self.sheet_names().len() as u32).find(|&s| self.sheet_id(s) == id)
            else {
                continue;
            };
            let list = self.sheet_validations(sheet);
            let patch = out.sheets.entry(id).or_default();
            patch
                .elements
                .push(("dataValidations", validations_xml(&list)));
            patch.remove_exts.push(crate::validation::DV_EXTENSION);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutting_ranges() {
        let a = CellRange::from_corners((0, 0), (9, 3));
        let b = CellRange::from_corners((2, 1), (4, 2));
        let parts = subtract(a, b);
        assert_eq!(parts.len(), 4);
        let cells: u32 = parts
            .iter()
            .map(|r| r.rows_count() * (r.right - r.left + 1))
            .sum();
        assert_eq!(cells, 40 - 6);
        assert!(parts.iter().all(|p| !p.intersects(&b)));
        assert_eq!(subtract(b, a), Vec::new());
    }

    #[test]
    fn operands() {
        assert_eq!(
            validation_operand(ValidationType::Date, "2026-01-31").unwrap(),
            "46053"
        );
        assert_eq!(
            validation_operand(ValidationType::Date, "31/01/2026").unwrap(),
            "46053"
        );
        assert_eq!(
            validation_operand_text(ValidationType::Date, "46053"),
            "2026-01-31"
        );
        assert_eq!(
            validation_operand(ValidationType::Time, "12:00").unwrap(),
            "0.5"
        );
        assert_eq!(
            validation_operand_text(ValidationType::Time, "0.5"),
            "12:00"
        );
        assert_eq!(
            validation_operand(ValidationType::Whole, "10").unwrap(),
            "10"
        );
        assert!(validation_operand(ValidationType::Whole, "1.5").is_err());
        assert_eq!(
            validation_operand(ValidationType::Decimal, "2,5").unwrap(),
            "2.5"
        );
        assert_eq!(
            validation_operand(ValidationType::Decimal, "=$B$1").unwrap(),
            "$B$1"
        );
        assert_eq!(list_source("Yes, No ,Maybe").unwrap(), "\"Yes,No,Maybe\"");
        assert_eq!(list_source("=$A$1:$A$3").unwrap(), "$A$1:$A$3");
        assert_eq!(list_source_text("\"Yes,No\""), "Yes, No");
        assert_eq!(list_source_text("Sheet2!$A$1:$A$3"), "=Sheet2!$A$1:$A$3");
    }
}
