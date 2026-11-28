//! Office's linear format (UnicodeMath, Unicode TN #28) as Word shows an
//! equation in one line: `a/b`, `x^2`, `√(x)`, `∑_(i=1)^n`. Writing
//! covers every construct; reading covers what people type in an
//! equation box (fractions, scripts, roots, big operators, brackets).

use crate::ast::*;
use crate::symbols;

/// An argument: in parentheses when it's more than one item.
fn arg(r: &[Node]) -> String {
    let s = write_row(r);
    if s.chars().count() <= 1 || (s.starts_with('(') && s.ends_with(')')) {
        s
    } else {
        format!("({s})")
    }
}

/// The linear form of an equation; display lines joined with spaces.
pub fn write(eq: &Equation) -> String {
    eq.lines
        .iter()
        .map(|l| write_row(l))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn write_row(r: &[Node]) -> String {
    r.iter().map(write_node).collect()
}

fn write_node(n: &Node) -> String {
    match n {
        Node::Text(r) => r.text.clone(),
        Node::Frac { kind, num, den } => match kind {
            FracKind::NoBar => format!("({}¦{})", write_row(num), write_row(den)),
            _ => format!("{}/{}", arg(num), arg(den)),
        },
        Node::Scripts { base, sub, sup } => {
            let mut s = arg(base);
            if let Some(r) = sub {
                s.push('_');
                s.push_str(&arg(r));
            }
            if let Some(r) = sup {
                s.push('^');
                s.push_str(&arg(r));
            }
            s
        }
        Node::PreScripts { sub, sup, base } => {
            format!("(_{}^{}){}", arg(sub), arg(sup), arg(base))
        }
        Node::Radical { degree, body } => {
            let deg = degree.as_ref().map(|d| write_row(d)).unwrap_or_default();
            match deg.as_str() {
                "" => format!("√{}", arg(body)),
                "3" => format!("∛{}", arg(body)),
                "4" => format!("∜{}", arg(body)),
                d => format!("√({d}&{})", write_row(body)),
            }
        }
        Node::Nary {
            op, sub, sup, body, ..
        } => {
            let mut s = op.to_string();
            if let Some(r) = sub.as_ref().filter(|r| !r.is_empty()) {
                s.push('_');
                s.push_str(&arg(r));
            }
            if let Some(r) = sup.as_ref().filter(|r| !r.is_empty()) {
                s.push('^');
                s.push_str(&arg(r));
            }
            format!("{s} {}", write_row(body))
        }
        Node::Delim {
            open,
            close,
            sep,
            items,
            ..
        } => {
            let inner: Vec<String> = items.iter().map(|i| write_row(i)).collect();
            format!("{open}{}{close}", inner.join(sep))
        }
        Node::Matrix { rows } => {
            let rows: Vec<String> = rows
                .iter()
                .map(|r| r.iter().map(|c| write_row(c)).collect::<Vec<_>>().join("&"))
                .collect();
            format!("■({})", rows.join("@"))
        }
        Node::EqArray { rows } => {
            let rows: Vec<String> = rows.iter().map(|r| write_row(r)).collect();
            format!("█({})", rows.join("@"))
        }
        Node::Accent { chr, body } => format!("{}{chr}", arg(body)),
        Node::Bar { top, body } => format!("{}{}", arg(body), if *top { '¯' } else { '▁' }),
        Node::Boxed { body } => format!("▭({})", write_row(body)),
        Node::Group { body } => write_row(body),
        Node::GroupChr { chr, body, .. } => format!("{chr}{}", arg(body)),
        Node::Limit { upper, base, limit } => {
            format!(
                "{}{}{}",
                arg(base),
                if *upper { '┴' } else { '┬' },
                arg(limit)
            )
        }
        Node::Func { name, body } => format!("{} {}", write_row(name), write_row(body)),
        Node::Phantom { body, show, .. } => {
            if *show {
                write_row(body)
            } else {
                String::new()
            }
        }
    }
}

// ── reading ────────────────────────────────────────────────────────────

/// Reads the linear format (the basics people type: `a/b`, `x^2`,
/// `x_(i+1)`, `√x`, `∑_(i=1)^n i`, `\alpha`).
pub fn parse(src: &str, display: bool) -> Equation {
    let chars: Vec<char> = src.chars().collect();
    let mut p = P { s: chars, i: 0 };
    let row = p.seq(None, false);
    Equation {
        display,
        justify: Justify::CenterGroup,
        lines: vec![row],
    }
}

struct P {
    s: Vec<char>,
    i: usize,
}

fn closing(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '⟨' => Some('⟩'),
        _ => None,
    }
}

impl P {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    /// Items up to `end` (consumed) or the end of input. An `operand`
    /// (of a big operator) also ends before a relation, and leaves `end`.
    fn seq(&mut self, end: Option<char>, operand: bool) -> Row {
        let mut items: Row = Vec::new();
        while let Some(c) = self.peek() {
            if Some(c) == end {
                if !operand {
                    self.i += 1;
                }
                break;
            }
            if operand && symbols::class(c) == symbols::Class::Rel {
                break;
            }
            match c {
                '^' | '_' => {
                    self.i += 1;
                    let a = self.operand();
                    let base = match items.pop() {
                        Some(Node::Scripts {
                            base,
                            sub: None,
                            sup,
                        }) if c == '_' => {
                            items.push(Node::Scripts {
                                base,
                                sub: Some(a),
                                sup,
                            });
                            continue;
                        }
                        Some(Node::Scripts {
                            base,
                            sub,
                            sup: None,
                        }) if c == '^' => {
                            items.push(Node::Scripts {
                                base,
                                sub,
                                sup: Some(a),
                            });
                            continue;
                        }
                        Some(Node::Nary {
                            op,
                            under_over,
                            sub,
                            sup,
                            body,
                        }) if body.is_empty() => {
                            let (sub, sup) = if c == '_' {
                                (Some(a), sup)
                            } else {
                                (sub, Some(a))
                            };
                            items.push(Node::Nary {
                                op,
                                under_over,
                                sub,
                                sup,
                                body,
                            });
                            continue;
                        }
                        Some(n) => vec![n],
                        None => Row::new(),
                    };
                    items.push(Node::Scripts {
                        base,
                        sub: (c == '_').then(|| a.clone()),
                        sup: (c == '^').then_some(a),
                    });
                }
                '/' => {
                    self.i += 1;
                    let num = match items.pop() {
                        Some(n) => strip(vec![n]),
                        None => Row::new(),
                    };
                    let den = self.operand();
                    items.push(Node::Frac {
                        kind: FracKind::Bar,
                        num,
                        den,
                    });
                }
                '√' | '∛' | '∜' => {
                    self.i += 1;
                    let degree = match c {
                        '∛' => Some(vec![Node::Text(Run::new("3"))]),
                        '∜' => Some(vec![Node::Text(Run::new("4"))]),
                        _ => None,
                    };
                    let body = self.operand();
                    items.push(Node::Radical { degree, body });
                }
                '▒' => {
                    // Operand of the big operator before.
                    self.i += 1;
                    let body = self.operand();
                    if let Some(Node::Nary { body: b, .. }) = items.last_mut() {
                        *b = body;
                    } else {
                        items.extend(body);
                    }
                }
                ' ' => {
                    self.i += 1;
                    // A space ends a big operator's limits: the rest up to a
                    // relation is its operand.
                    if let Some(Node::Nary { body, .. }) = items.last_mut()
                        && body.is_empty()
                    {
                        *body = self.seq(end, true);
                    }
                }
                c if symbols::is_nary(c) => {
                    self.i += 1;
                    items.push(Node::Nary {
                        op: c,
                        under_over: !symbols::is_integral(c),
                        sub: None,
                        sup: None,
                        body: Row::new(),
                    });
                }
                _ => {
                    let u = self.unit();
                    items.extend(u);
                }
            }
        }
        normalize(items)
    }

    /// An operand after `/`, `^`, `_`, `√`: one unit, its parentheses
    /// dropped.
    fn operand(&mut self) -> Row {
        let u = self.unit();
        strip(u)
    }

    /// One unit: a bracketed group, a run of letters and digits, a
    /// `\name`, or one character.
    fn unit(&mut self) -> Row {
        let Some(c) = self.peek() else {
            return Row::new();
        };
        if let Some(close) = closing(c) {
            self.i += 1;
            let inner = self.seq(Some(close), false);
            return vec![Node::Delim {
                open: c.to_string(),
                close: close.to_string(),
                sep: "|".into(),
                items: vec![inner],
                grow: true,
            }];
        }
        if c == '\\' {
            self.i += 1;
            let mut name = String::new();
            while let Some(c) = self.peek() {
                if c.is_ascii_alphabetic() {
                    name.push(c);
                    self.i += 1;
                } else {
                    break;
                }
            }
            if let Some(ch) = symbols::symbol(&name) {
                return vec![Node::Text(Run::new(ch.to_string()))];
            }
            return vec![Node::Text(Run::new(format!("\\{name}")))];
        }
        if c.is_alphanumeric() || c == '.' {
            let mut s = String::new();
            while let Some(c) = self.peek() {
                if c.is_alphanumeric() || (c == '.' && !s.is_empty()) {
                    s.push(c);
                    self.i += 1;
                } else {
                    break;
                }
            }
            if s.is_empty() {
                self.i += 1;
                s.push(c);
            }
            if symbols::FUNCTIONS.contains(&s.as_str()) {
                // A function: its argument is the next unit.
                while self.peek() == Some(' ') {
                    self.i += 1;
                }
                let body = if self
                    .peek()
                    .is_some_and(|c| c.is_alphanumeric() || closing(c).is_some() || c == '\\')
                {
                    self.unit()
                } else {
                    Row::new()
                };
                return vec![Node::Func {
                    name: vec![Node::Text(Run::styled(s, Style::PLAIN))],
                    body,
                }];
            }
            return vec![Node::Text(Run::new(s))];
        }
        self.i += 1;
        let c = if c == '-' { '−' } else { c };
        vec![Node::Text(Run::new(c.to_string()))]
    }
}

/// An operand's outer parentheses dropped (`(a+b)/2`).
fn strip(r: Row) -> Row {
    match r.as_slice() {
        [
            Node::Delim {
                open, close, items, ..
            },
        ] if open == "(" && close == ")" && items.len() == 1 => items[0].clone(),
        _ => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_basics() {
        let eq = parse("(a+b)/2", false);
        assert!(matches!(&eq.lines[0][0], Node::Frac { num, .. } if write_row(num) == "a+b"));
        let eq = parse("x^10+y_(i+1)", false);
        assert_eq!(write(&eq), "x^(10)+y_(i+1)");
        let eq = parse("√(b^2−4ac)", false);
        assert_eq!(write(&eq), "√(b^2−4ac)");
        let eq = parse("∑_(i=1)^n i^2", true);
        assert_eq!(write(&eq), "∑_(i=1)^n i^2");
    }
}
