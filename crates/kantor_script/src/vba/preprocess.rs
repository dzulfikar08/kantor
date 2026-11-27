//! Source clean-up before lexing: line endings, the `VERSION`/`BEGIN … END` header of exported
//! `.cls`/`.frm` files, and conditional compilation (`#If`/`#Else`/`#End If`/`#Const`,
//! MS-VBAL §3.4). Removed lines become blank so line numbers stay right.
//!
//! The predefined compilation constants describe 64-bit Office on Windows (`VBA7`, `Win64`,
//! `Win32` true, `Mac` false): code behind `#If Mac` usually calls AppleScript, and `Declare`
//! statements fail with a clear error either way.

use std::collections::HashMap;

use super::ast::{Expr, UnOp};
use super::convert;
use super::lexer::tokenize;
use super::parser::Parser;
use crate::object::{Location, ScriptError, Value};

type R<T> = Result<T, ScriptError>;

struct Branch {
    /// The enclosing region is active.
    parent: bool,
    /// Some branch of this `#If` was taken already.
    taken: bool,
    /// Lines in the current branch are kept.
    active: bool,
}

fn syntax(line: usize, msg: &str) -> ScriptError {
    let mut e = ScriptError::syntax(msg);
    e.location = Some(Location {
        module: String::new(),
        procedure: String::new(),
        line: line as u32 + 1,
    });
    e
}

pub fn preprocess(src: &str) -> R<String> {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
    skip_header(&mut lines);
    let mut consts: HashMap<String, Value> = [
        ("vba6", true),
        ("vba7", true),
        ("win16", false),
        ("win32", true),
        ("win64", true),
        ("mac", false),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::Boolean(v)))
    .collect();
    let mut stack: Vec<Branch> = Vec::new();
    let mut out = String::with_capacity(normalized.len());
    for (n, line) in lines.iter().enumerate() {
        let active = stack.last().is_none_or(|b| b.active);
        let trimmed = line.trim_start();
        if let Some(directive) = trimmed.strip_prefix('#') {
            let lower = directive.trim().to_ascii_lowercase();
            let word = lower.split_whitespace().next().unwrap_or("");
            match word {
                "if" => {
                    let cond = if active {
                        truthy(&eval_condition(directive, n, &consts)?)
                    } else {
                        false
                    };
                    stack.push(Branch {
                        parent: active,
                        taken: cond,
                        active: active && cond,
                    });
                }
                "elseif" => {
                    let Some(top) = stack.last() else {
                        return Err(syntax(n, "#ElseIf without #If"));
                    };
                    let (parent, taken) = (top.parent, top.taken);
                    let cond = parent && !taken && truthy(&eval_condition(directive, n, &consts)?);
                    let top = stack.last_mut().expect("checked above");
                    top.active = cond;
                    top.taken |= cond;
                }
                "else" => {
                    let Some(top) = stack.last_mut() else {
                        return Err(syntax(n, "#Else without #If"));
                    };
                    top.active = top.parent && !top.taken;
                    top.taken = true;
                }
                "end" | "endif" => {
                    if stack.pop().is_none() {
                        return Err(syntax(n, "#End If without #If"));
                    }
                }
                "const" => {
                    if active {
                        let body = directive.trim()[5..].trim();
                        let Some((name, expr)) = body.split_once('=') else {
                            return Err(syntax(n, "Expected '=' in #Const"));
                        };
                        let value = eval_text(expr, n, &consts)?;
                        consts.insert(name.trim().to_ascii_lowercase(), value);
                    }
                }
                _ => return Err(syntax(n, "Unknown compiler directive")),
            }
            out.push('\n');
            continue;
        }
        if active {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !stack.is_empty() {
        return Err(syntax(lines.len().saturating_sub(1), "#If without #End If"));
    }
    Ok(out)
}

/// Blanks the `VERSION 1.0 CLASS` / `BEGIN … END` header of exported class and form modules.
fn skip_header(lines: &mut [String]) {
    let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return;
    };
    if !lines[first]
        .trim_start()
        .to_ascii_uppercase()
        .starts_with("VERSION ")
    {
        return;
    }
    lines[first].clear();
    let mut depth = 0usize;
    for line in lines.iter_mut().skip(first + 1) {
        let upper = line.trim().to_ascii_uppercase();
        let opens =
            upper == "BEGIN" || upper.starts_with("BEGIN ") || upper.starts_with("BEGINPROPERTY");
        let closes = upper == "END" || upper.starts_with("ENDPROPERTY");
        if depth == 0 && !opens {
            break;
        }
        if opens {
            depth += 1;
        } else if closes {
            depth = depth.saturating_sub(1);
        }
        line.clear();
        if depth == 0 {
            break;
        }
    }
}

fn truthy(v: &Value) -> bool {
    convert::to_bool(v).unwrap_or(false)
}

/// `If cond Then` / `ElseIf cond Then` after the `#`.
fn eval_condition(directive: &str, line: usize, consts: &HashMap<String, Value>) -> R<Value> {
    let text = directive.trim();
    let after = text
        .split_once(char::is_whitespace)
        .map_or("", |(_, rest)| rest);
    let lower = after.to_ascii_lowercase();
    let cond = match lower.rfind("then") {
        Some(i) => &after[..i],
        None => after,
    };
    eval_text(cond, line, consts)
}

fn eval_text(text: &str, line: usize, consts: &HashMap<String, Value>) -> R<Value> {
    let toks = tokenize(text).map_err(|e| syntax(line, &e.description))?;
    let mut p = Parser::new(toks);
    let e = p.expr().map_err(|e| syntax(line, &e.description))?;
    eval(&e, consts).map_err(|e| syntax(line, &e.description))
}

fn eval(e: &Expr, consts: &HashMap<String, Value>) -> R<Value> {
    match e {
        Expr::Lit(v) => Ok(v.clone()),
        Expr::Name(id) => Ok(consts.get(&*id.key).cloned().unwrap_or(Value::Empty)),
        Expr::Paren(x) => eval(x, consts),
        Expr::Unary(UnOp::Neg, x) => convert::negate(&eval(x, consts)?),
        Expr::Unary(UnOp::Not, x) => convert::not(&eval(x, consts)?),
        Expr::Binary(op, a, b) => convert::binary(*op, &eval(a, consts)?, &eval(b, consts)?, false),
        _ => Err(ScriptError::syntax(
            "Only constants and operators are allowed in #If",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_compilation() {
        let src = "#Const DEV = 1\r\n#If VBA7 And Win64 Then\r\na = 1\r\n#ElseIf Mac Then\r\na = 2\r\n#Else\r\na = 3\r\n#End If\r\n#If DEV = 0 Then\r\nb = 1\r\n#End If\r\n";
        let out = preprocess(src).unwrap();
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines[2], "a = 1");
        assert_eq!(lines[4], "");
        assert_eq!(lines[6], "");
        assert_eq!(lines[9], "");
    }

    #[test]
    fn class_header() {
        let src = "VERSION 1.0 CLASS\nBEGIN\n  MultiUse = -1  'True\nEND\nAttribute VB_Name = \"Class1\"\nPublic X As Long\n";
        let out = preprocess(src).unwrap();
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(&lines[..4], &["", "", "", ""]);
        assert_eq!(lines[5], "Public X As Long");
    }
}
