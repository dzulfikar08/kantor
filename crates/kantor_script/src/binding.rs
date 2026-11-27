//! What the JavaScript and Python bindings (`kantor_script_js`, `kantor_script_py`) share.
//!
//! Both languages separate reading a property (`r.value`) from calling a method (`r.clear()`),
//! and a property access can't tell whether a call follows. [`member`] decides what naming a
//! member gives: a method (a callable that runs it with the call's arguments) when the host
//! says so ([`crate::Host::is_method`]) or when reading it without arguments fails for want of them
//! (449 "Argument not optional", 450 "Wrong number of arguments"), else the property's value,
//! read right away.

use crate::{ObjectRef, ScriptError, Value};

/// What naming member `name` of an object gives.
#[derive(Debug)]
pub enum Member {
    /// A property's value, already read.
    Value(Value),
    /// A method: call it to run it.
    Method,
}

/// Resolves member `name` of `obj` (see the module docs). `is_method`: what
/// [`crate::Host::is_method`] says of it.
pub fn member(obj: &ObjectRef, name: &str, is_method: bool) -> Result<Member, ScriptError> {
    if is_method {
        return Ok(Member::Method);
    }
    match obj.get(name, &[]) {
        Ok(v) => Ok(Member::Value(v)),
        Err(e) if needs_arguments(&e) => Ok(Member::Method),
        Err(e) => Err(e),
    }
}

/// Whether `e` says arguments were missing.
pub fn needs_arguments(e: &ScriptError) -> bool {
    e.kind == crate::ErrorKind::Runtime && matches!(e.number, 449 | 450)
}

/// VBA's constants for `MsgBox` and text (`vbYesNo`, `vbYes`, `vbCrLf`), which scripts in
/// other languages use with `MsgBox` too. Any case.
pub fn vb_constant(name: &str) -> Option<Value> {
    let n = match name.to_ascii_lowercase().as_str() {
        "vbokonly" | "vbdefaultbutton1" => 0,
        "vbokcancel" | "vbok" => 1,
        "vbabortretryignore" | "vbcancel" => 2,
        "vbyesnocancel" | "vbabort" => 3,
        "vbyesno" | "vbretry" => 4,
        "vbretrycancel" | "vbignore" => 5,
        "vbyes" => 6,
        "vbno" => 7,
        "vbcritical" => 16,
        "vbquestion" => 32,
        "vbexclamation" => 48,
        "vbinformation" => 64,
        "vbdefaultbutton2" => 256,
        "vbdefaultbutton3" => 512,
        "vbcrlf" | "vbnewline" => return Some(Value::from("\r\n")),
        "vblf" => return Some(Value::from("\n")),
        "vbcr" => return Some(Value::from("\r")),
        "vbtab" => return Some(Value::from("\t")),
        _ => return None,
    };
    Some(Value::Long(n))
}

/// A top-level function of a script module, as the Macros dialog lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionInfo {
    pub name: String,
    /// The number of parameters (a macro has none).
    pub params: usize,
    /// The line of its definition (0-based).
    pub line: usize,
    /// Its last line (0-based): the line before the next top-level definition, or the end.
    pub end_line: usize,
}

impl FunctionInfo {
    /// What Alt+F8 lists: no parameters, not private (a name starting with `_`).
    pub fn is_macro(&self) -> bool {
        self.params == 0 && !self.name.starts_with('_')
    }
}

/// `Module.function` or `function`.
pub fn split_target(target: &str) -> (Option<&str>, &str) {
    match target.rsplit_once('.') {
        Some((m, f)) => (Some(m.trim()), f.trim()),
        None => (None, target.trim()),
    }
}

/// The number of parameters in a parameter list's text (`a, b = 1, *rest` is 3).
pub fn count_params(list: &str) -> usize {
    let mut depth = 0i32;
    let mut count = 0;
    let mut any = false;
    for c in list.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                if any {
                    count += 1;
                }
                any = false;
                continue;
            }
            _ => {}
        }
        if !c.is_whitespace() {
            any = true;
        }
    }
    count + usize::from(any)
}

/// The functions a Python module defines at its top level (`def go():`, `async def f(x):`
/// at the start of a line), in order.
pub fn python_functions(source: &str) -> Vec<FunctionInfo> {
    let mut found = Vec::new();
    let mut lines = 0;
    for (i, line) in source.lines().enumerate() {
        lines = i + 1;
        let rest = line
            .strip_prefix("async ")
            .map(str::trim_start)
            .unwrap_or(line);
        let Some(rest) = rest.strip_prefix("def") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let Some(list) = rest[name.len()..].trim_start().strip_prefix('(') else {
            continue;
        };
        let params = match list.find(')') {
            Some(end) => count_params(&list[..end]),
            None => 1,
        };
        found.push(FunctionInfo {
            name,
            params,
            line: i,
            end_line: i,
        });
    }
    set_end_lines(&mut found, lines);
    found
}

/// Fills in each function's `end_line` from where the next one starts (`lines` in the
/// module).
pub fn set_end_lines(found: &mut [FunctionInfo], lines: usize) {
    let starts: Vec<usize> = found.iter().map(|f| f.line).collect();
    for (i, f) in found.iter_mut().enumerate() {
        let next = starts.get(i + 1).copied().unwrap_or(lines);
        f.end_line = next.saturating_sub(1).max(f.line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params() {
        assert_eq!(count_params(""), 0);
        assert_eq!(count_params("  "), 0);
        assert_eq!(count_params("a"), 1);
        assert_eq!(count_params("a, b = [1, 2], *rest"), 3);
        assert_eq!(count_params("{ a, b }, c"), 2);
        assert_eq!(count_params("a,"), 1);
    }

    #[test]
    fn python_top_level_functions() {
        let src = "import math\n\ndef go():\n    def inner(): pass\n    return 1\n\nasync def fetch(url):\n    pass\nclass A:\n    def m(self): pass\ndef _private():\n    pass\n";
        let f: Vec<(String, usize, bool, usize, usize)> = python_functions(src)
            .into_iter()
            .map(|x| {
                let m = x.is_macro();
                (x.name, x.params, m, x.line, x.end_line)
            })
            .collect();
        assert_eq!(
            f,
            vec![
                ("go".to_owned(), 0, true, 2, 5),
                ("fetch".to_owned(), 1, false, 6, 9),
                ("_private".to_owned(), 0, false, 10, 11),
            ]
        );
    }

    #[test]
    fn targets() {
        assert_eq!(split_target("Report.go"), (Some("Report"), "go"));
        assert_eq!(split_target("go"), (None, "go"));
    }
}
