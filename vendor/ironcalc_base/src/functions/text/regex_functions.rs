// KANTOR PATCH (section 36): REGEXTEST, REGEXEXTRACT and REGEXREPLACE take
// Excel's optional arguments, run on `fancy-regex` (lookaround and
// backreferences, as in Excel's PCRE2) and work element-wise over ranges.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fancy_regex::{Regex, RegexBuilder};

use super::common::{broadcast_text, TextArg};
use crate::{
    calc_result::CalcResult,
    cast::array_node_to_string,
    expressions::{
        parser::{ArrayNode, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    model::Model,
};

/// Compiled patterns, so a formula over a long range compiles each pattern once.
#[derive(Default)]
struct Patterns(RefCell<HashMap<(String, bool), Option<Rc<Regex>>>>);

impl Patterns {
    /// The pattern `pattern`, case insensitive when `ignore_case`; `#VALUE!`
    /// when it is not a valid regular expression.
    fn get(&self, pattern: &str, ignore_case: bool) -> Result<Rc<Regex>, Error> {
        let key = (pattern.to_string(), ignore_case);
        if let Some(found) = self.0.borrow().get(&key) {
            return found.clone().ok_or(Error::VALUE);
        }
        let compiled = RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .ok()
            .map(Rc::new);
        self.0.borrow_mut().insert(key, compiled.clone());
        compiled.ok_or(Error::VALUE)
    }
}

/// A whole number from an element (booleans count as 0/1, empty as 0).
fn whole_number(node: &ArrayNode) -> Result<i64, Error> {
    let value = match node {
        ArrayNode::Number(n) => *n,
        ArrayNode::Boolean(b) => f64::from(u8::from(*b)),
        ArrayNode::Empty => 0.0,
        ArrayNode::String(s) => s.trim().parse::<f64>().map_err(|_| Error::VALUE)?,
        ArrayNode::Error(e) => return Err(e.clone()),
    };
    if !value.is_finite() {
        return Err(Error::VALUE);
    }
    Ok(value.trunc() as i64)
}

/// The `case_sensitivity` argument: 0 (the default) is case sensitive, 1 is not.
fn ignore_case(node: &ArrayNode) -> Result<bool, Error> {
    match whole_number(node)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::VALUE),
    }
}

/// Excel's replacement text (`$n`, `${n}`, `$$`) written for `fancy-regex`,
/// where `$1a` would name a group "1a": a `$` and its digits become `${n}`,
/// and a `$` that starts no reference is a literal dollar sign.
fn replacement_template(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len() + 4);
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            Some(d) if d.is_ascii_digit() => {
                out.push_str("${");
                while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                    out.push(d);
                    chars.next();
                }
                out.push('}');
            }
            Some('{') => {
                // `${name}`: kept as it is when it closes, else a literal `$`.
                let rest: String = chars.clone().collect();
                if rest.contains('}') {
                    out.push('$');
                } else {
                    out.push_str("$$");
                }
            }
            _ => out.push_str("$$"),
        }
    }
    out
}

impl<'a> Model<'a> {
    /// The arguments of a regex function, each one possibly a range or an array.
    fn regex_args(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> Result<Vec<TextArg>, CalcResult> {
        args.iter().map(|a| self.text_arg(a, cell)).collect()
    }

    /// REGEXTEST(text, pattern, [case_sensitivity])
    ///
    /// `TRUE` when `pattern` matches anywhere in `text`.
    pub(crate) fn fn_regextest(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() < 2 || args.len() > 3 {
            return CalcResult::new_args_number_error(cell);
        }
        let mut a = match self.regex_args(args, cell) {
            Ok(a) => a,
            Err(e) => return e,
        };
        if a.len() == 2 {
            a.push(TextArg::Scalar(ArrayNode::Number(0.0)));
        }
        let patterns = Patterns::default();
        let operands: Vec<&TextArg> = a.iter().collect();
        broadcast_text(cell, &operands, |i, j| {
            let text = array_node_to_string(a[0].elem(i, j))?;
            let pattern = array_node_to_string(a[1].elem(i, j))?;
            let re = patterns.get(&pattern, ignore_case(a[2].elem(i, j))?)?;
            let found = re.is_match(text.as_str()).map_err(|_| Error::VALUE)?;
            Ok(ArrayNode::Boolean(found))
        })
    }

    /// REGEXEXTRACT(text, pattern, [return_mode], [case_sensitivity])
    ///
    /// `return_mode` 0 (the default) gives the first match, 1 every match and
    /// 2 the capturing groups of the first match; 1 and 2 give a row. No match
    /// is `#N/A`; a group that took no part in the match is empty text.
    pub(crate) fn fn_regexextract(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 2 || args.len() > 4 {
            return CalcResult::new_args_number_error(cell);
        }
        let mut a = match self.regex_args(args, cell) {
            Ok(a) => a,
            Err(e) => return e,
        };
        while a.len() < 4 {
            a.push(TextArg::Scalar(ArrayNode::Number(0.0)));
        }
        let patterns = Patterns::default();
        // One result per element: a row for modes 1 and 2.
        let extract = |i: usize, j: usize| -> Result<Vec<ArrayNode>, Error> {
            let text = array_node_to_string(a[0].elem(i, j))?;
            let pattern = array_node_to_string(a[1].elem(i, j))?;
            let mode = whole_number(a[2].elem(i, j))?;
            let re = patterns.get(&pattern, ignore_case(a[3].elem(i, j))?)?;
            let row: Vec<ArrayNode> = match mode {
                0 => match re.find(text.as_str()).map_err(|_| Error::VALUE)? {
                    Some(m) => vec![ArrayNode::String(m.as_str().to_string())],
                    None => vec![],
                },
                1 => {
                    let mut all = Vec::new();
                    for m in re.find_iter(text.as_str()) {
                        let m = m.map_err(|_| Error::VALUE)?;
                        all.push(ArrayNode::String(m.as_str().to_string()));
                    }
                    all
                }
                2 => match re.captures(text.as_str()).map_err(|_| Error::VALUE)? {
                    Some(caps) if caps.len() > 1 => (1..caps.len())
                        .map(|g| {
                            ArrayNode::String(
                                caps.get(g)
                                    .map_or(String::new(), |m| m.as_str().to_string()),
                            )
                        })
                        .collect(),
                    // A pattern without groups gives the whole match.
                    Some(caps) => vec![ArrayNode::String(
                        caps.get(0)
                            .map_or(String::new(), |m| m.as_str().to_string()),
                    )],
                    None => vec![],
                },
                _ => return Err(Error::VALUE),
            };
            if row.is_empty() {
                return Err(Error::NA);
            }
            Ok(row)
        };

        let all_scalar = a.iter().all(|t| t.dims() == (1, 1));
        if all_scalar {
            for t in &a {
                if let TextArg::Error(_, error) = t {
                    return error.clone();
                }
            }
            return match extract(0, 0) {
                Ok(row) if row.len() == 1 => match row.into_iter().next() {
                    Some(ArrayNode::String(s)) => CalcResult::String(s),
                    _ => CalcResult::String(String::new()),
                },
                Ok(row) => CalcResult::Array(vec![row]),
                Err(e) => CalcResult::new_error(e, cell, "No match found".to_string()),
            };
        }
        // Over a range each element gives one value; a row of several values
        // cannot sit in one element (Excel's nested arrays are #CALC!).
        let operands: Vec<&TextArg> = a.iter().collect();
        broadcast_text(cell, &operands, |i, j| {
            let row = extract(i, j)?;
            if row.len() == 1 {
                Ok(row.into_iter().next().unwrap_or(ArrayNode::Empty))
            } else {
                Err(Error::CALC)
            }
        })
    }

    /// REGEXREPLACE(text, pattern, replacement, [occurrence], [case_sensitivity])
    ///
    /// `occurrence` 0 (the default) replaces every match, n the n-th one and
    /// -n the n-th from the end; a match that isn't there leaves `text` as it is.
    /// `replacement` refers to groups as `$n`.
    pub(crate) fn fn_regexreplace(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 3 || args.len() > 5 {
            return CalcResult::new_args_number_error(cell);
        }
        let mut a = match self.regex_args(args, cell) {
            Ok(a) => a,
            Err(e) => return e,
        };
        while a.len() < 5 {
            a.push(TextArg::Scalar(ArrayNode::Number(0.0)));
        }
        let patterns = Patterns::default();
        let operands: Vec<&TextArg> = a.iter().collect();
        broadcast_text(cell, &operands, |i, j| {
            let text = array_node_to_string(a[0].elem(i, j))?;
            let pattern = array_node_to_string(a[1].elem(i, j))?;
            let template = replacement_template(&array_node_to_string(a[2].elem(i, j))?);
            let occurrence = whole_number(a[3].elem(i, j))?;
            let re = patterns.get(&pattern, ignore_case(a[4].elem(i, j))?)?;
            let mut matches = Vec::new();
            for caps in re.captures_iter(text.as_str()) {
                matches.push(caps.map_err(|_| Error::VALUE)?);
            }
            let chosen: Vec<usize> = if occurrence == 0 {
                (0..matches.len()).collect()
            } else {
                let n = occurrence.unsigned_abs() as usize;
                if n > matches.len() {
                    vec![]
                } else if occurrence > 0 {
                    vec![n - 1]
                } else {
                    vec![matches.len() - n]
                }
            };
            let mut out = String::with_capacity(text.len());
            let mut last = 0;
            for k in chosen {
                let caps = &matches[k];
                let Some(whole) = caps.get(0) else { continue };
                out.push_str(&text[last..whole.start()]);
                caps.expand(&template, &mut out);
                last = whole.end();
            }
            out.push_str(&text[last..]);
            Ok(ArrayNode::String(out))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::replacement_template;

    #[test]
    fn replacement_dollars() {
        assert_eq!(replacement_template("$2, $1"), "${2}, ${1}");
        assert_eq!(replacement_template("$1a"), "${1}a");
        assert_eq!(replacement_template("$$5"), "$$5");
        assert_eq!(replacement_template("US$ 5"), "US$$ 5");
        assert_eq!(replacement_template("${name}!"), "${name}!");
        assert_eq!(replacement_template("cost $"), "cost $$");
        assert_eq!(replacement_template("${open"), "$${open");
    }
}
