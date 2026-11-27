//! A light line scanner for `Sub`, `Function` and `Property` headers in
//! module source. Not a parser: it reads declaration lines (joined across
//! ` _` continuations) and their `End` lines, which is enough to list
//! procedures and scroll to them.

/// What kind of procedure a header declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcedureKind {
    Sub,
    Function,
    PropertyGet,
    PropertyLet,
    PropertySet,
}

impl ProcedureKind {
    /// As VBA writes it: `Sub`, `Function`, `Property Get`, …
    pub fn keyword(self) -> &'static str {
        match self {
            ProcedureKind::Sub => "Sub",
            ProcedureKind::Function => "Function",
            ProcedureKind::PropertyGet => "Property Get",
            ProcedureKind::PropertyLet => "Property Let",
            ProcedureKind::PropertySet => "Property Set",
        }
    }
}

/// A procedure in a module's code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Procedure {
    pub name: String,
    pub kind: ProcedureKind,
    /// `Private` (otherwise `Public` or `Friend`, or nothing: public).
    pub private: bool,
    /// Its parameter list is not empty.
    pub has_arguments: bool,
    /// The header's first line, counting from 0 in the code it was read
    /// from.
    pub line: usize,
    /// The `End …` line (the last line of the code when there is none).
    pub end_line: usize,
}

impl Procedure {
    /// Whether Excel lists it in the Macros dialog (Alt+F8): a public `Sub`
    /// without arguments (in a standard or document module).
    pub fn is_macro(&self) -> bool {
        self.kind == ProcedureKind::Sub && !self.private && !self.has_arguments
    }
}

/// The procedures declared in `code`, in order.
pub fn scan(code: &str) -> Vec<Procedure> {
    let lines: Vec<&str> = code.lines().collect();
    let mut out: Vec<Procedure> = Vec::new();
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let first = i;
        // A logical line: physical lines joined at a trailing ` _`.
        let mut logical = String::from(strip_comment(lines[i]).trim_end());
        while logical.ends_with(" _") && i + 1 < lines.len() {
            logical.truncate(logical.len() - 2);
            logical.push(' ');
            i += 1;
            logical.push_str(strip_comment(lines[i]).trim());
        }
        i += 1;
        let text = logical.trim();
        if let Some(at) = open {
            if is_end(text, out[at].kind) {
                out[at].end_line = i - 1;
                open = None;
            }
            continue;
        }
        if let Some(p) = header(text, first) {
            out.push(p);
            open = Some(out.len() - 1);
        }
    }
    if let Some(at) = open {
        out[at].end_line = lines.len().saturating_sub(1);
    }
    out
}

/// `line` without a comment (`'` outside a string, or `Rem`).
fn strip_comment(line: &str) -> &str {
    let t = line.trim_start();
    if starts_word(t, "Rem") {
        return "";
    }
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '\'' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `text` starts with the word `word` (case-insensitive, followed by a
/// space, `(` or the end).
fn starts_word(text: &str, word: &str) -> bool {
    text.len() >= word.len()
        && text.is_char_boundary(word.len())
        && text[..word.len()].eq_ignore_ascii_case(word)
        && text[word.len()..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == '(')
}

/// `text` after the word `word` and spaces.
fn after_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    starts_word(text, word).then(|| text[word.len()..].trim_start())
}

fn is_end(text: &str, kind: ProcedureKind) -> bool {
    let Some(rest) = after_word(text, "End") else {
        return false;
    };
    let word = match kind {
        ProcedureKind::Sub => "Sub",
        ProcedureKind::Function => "Function",
        _ => "Property",
    };
    starts_word(rest, word)
}

fn header(text: &str, line: usize) -> Option<Procedure> {
    let mut rest = text;
    let mut private = false;
    loop {
        if let Some(r) = after_word(rest, "Private") {
            private = true;
            rest = r;
        } else if let Some(r) = after_word(rest, "Public")
            .or_else(|| after_word(rest, "Friend"))
            .or_else(|| after_word(rest, "Static"))
        {
            rest = r;
        } else {
            break;
        }
    }
    const KINDS: [(&str, Option<&str>, ProcedureKind); 5] = [
        ("Sub", None, ProcedureKind::Sub),
        ("Function", None, ProcedureKind::Function),
        ("Property", Some("Get"), ProcedureKind::PropertyGet),
        ("Property", Some("Let"), ProcedureKind::PropertyLet),
        ("Property", Some("Set"), ProcedureKind::PropertySet),
    ];
    let (kind, rest) = KINDS.iter().find_map(|(word, second, kind)| {
        let r = after_word(rest, word)?;
        match second {
            Some(w) => Some((*kind, after_word(r, w)?)),
            None => Some((*kind, r)),
        }
    })?;
    let name_end = rest
        .find(|c: char| c == '(' || c.is_whitespace())
        .unwrap_or(rest.len());
    let name = rest[..name_end].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let has_arguments = rest[name_end..]
        .trim_start()
        .strip_prefix('(')
        .and_then(|r| r.find(')').map(|end| !r[..end].trim().is_empty()))
        .unwrap_or(false);
    Some(Procedure {
        name: name.to_owned(),
        kind,
        private,
        has_arguments,
        line,
        end_line: line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn headers_and_ends() {
        let code = "Option Explicit\n\
            ' Sub NotThis()\n\
            Sub Hello()\n\
            \x20   MsgBox \"End Sub\" ' End Sub\n\
            End Sub\n\
            Private Function Twice(ByVal x As Long) As Long\n\
            \x20   Twice = x * 2\n\
            End Function\n\
            Public Static Sub Counter( _\n\
            \x20   )\n\
            End Sub\n\
            Property Get Total() As Double\n\
            End Property\n\
            Private Declare PtrSafe Function GetTickCount Lib \"kernel32\" () As Long\n\
            Friend Property Let Name(v As String)\n\
            End Property\n";
        let p = scan(code);
        let names: Vec<_> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Hello", "Twice", "Counter", "Total", "Name"]);
        assert_eq!((p[0].line, p[0].end_line), (2, 4));
        assert!(p[0].is_macro());
        assert!(p[1].private && p[1].has_arguments && !p[1].is_macro());
        assert_eq!(p[1].kind, ProcedureKind::Function);
        assert_eq!((p[2].line, p[2].end_line), (8, 10));
        assert!(!p[2].has_arguments && p[2].is_macro());
        assert_eq!(p[3].kind, ProcedureKind::PropertyGet);
        assert_eq!(p[4].kind, ProcedureKind::PropertyLet);
        assert!(p[4].has_arguments);
    }
}
