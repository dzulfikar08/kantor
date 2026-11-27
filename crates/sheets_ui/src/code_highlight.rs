//! Syntax colouring for the Macro Editor: a small lexer for VBA and
//! JavaScript (keywords, strings, comments, numbers) behind gpui-base's
//! code editor ([`InputHighlighter`]). Macros are short: every change
//! re-lexes the whole text.

use std::ops::Range;

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::*;

/// What a run of code is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
}

impl TokenKind {
    /// The name the theme's syntax styles go by.
    fn style_name(self) -> &'static str {
        match self {
            TokenKind::Keyword => "keyword",
            TokenKind::String => "string",
            TokenKind::Comment => "comment",
            TokenKind::Number => "number",
        }
    }

    /// Used when the theme has no style of that name.
    fn fallback(self) -> HighlightStyle {
        let color = match self {
            TokenKind::Keyword => rgb(0x2f5bd3),
            TokenKind::String => rgb(0xb2432c),
            TokenKind::Comment => rgb(0x3a8a3a),
            TokenKind::Number => rgb(0x098658),
        };
        HighlightStyle {
            color: Some(color.into()),
            ..Default::default()
        }
    }
}

/// The languages the editor colours, by the names its states use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeLanguage {
    Vba,
    JavaScript,
    /// Kept and shown, coloured like JavaScript's comments-and-strings
    /// subset is not worth it: Python is plain.
    Plain,
}

impl CodeLanguage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            CodeLanguage::Vba => "kantor-vba",
            CodeLanguage::JavaScript => "kantor-js",
            CodeLanguage::Plain => "kantor-plain",
        }
    }

    fn from_name(name: &str) -> Option<CodeLanguage> {
        match name {
            "kantor-vba" => Some(CodeLanguage::Vba),
            "kantor-js" => Some(CodeLanguage::JavaScript),
            "kantor-plain" => Some(CodeLanguage::Plain),
            _ => None,
        }
    }
}

const VBA_KEYWORDS: &[&str] = &[
    "addressof",
    "and",
    "as",
    "attribute",
    "base",
    "binary",
    "boolean",
    "byref",
    "byte",
    "byval",
    "call",
    "case",
    "compare",
    "const",
    "currency",
    "date",
    "declare",
    "defbool",
    "defdbl",
    "defint",
    "deflng",
    "defstr",
    "defvar",
    "dim",
    "do",
    "double",
    "each",
    "else",
    "elseif",
    "empty",
    "end",
    "enum",
    "eqv",
    "erase",
    "error",
    "event",
    "exit",
    "explicit",
    "false",
    "for",
    "friend",
    "function",
    "get",
    "global",
    "gosub",
    "goto",
    "if",
    "imp",
    "implements",
    "in",
    "integer",
    "is",
    "let",
    "lib",
    "like",
    "long",
    "longlong",
    "loop",
    "lset",
    "me",
    "mod",
    "module",
    "new",
    "next",
    "not",
    "nothing",
    "null",
    "object",
    "on",
    "option",
    "optional",
    "or",
    "paramarray",
    "preserve",
    "private",
    "property",
    "ptrsafe",
    "public",
    "raiseevent",
    "redim",
    "resume",
    "return",
    "rset",
    "select",
    "set",
    "single",
    "static",
    "step",
    "stop",
    "string",
    "sub",
    "text",
    "then",
    "to",
    "true",
    "type",
    "typeof",
    "until",
    "variant",
    "wend",
    "while",
    "with",
    "withevents",
    "xor",
];

const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// The coloured runs of `text`, in order (byte ranges).
pub(crate) fn tokens(text: &str, language: CodeLanguage) -> Vec<(Range<usize>, TokenKind)> {
    match language {
        CodeLanguage::Vba => vba_tokens(text),
        CodeLanguage::JavaScript => js_tokens(text),
        CodeLanguage::Plain => Vec::new(),
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// The end of a number starting at `i` (digits, `.`, an exponent, a type
/// suffix).
fn number_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    i
}

/// The last character before `i` that isn't a space or tab.
fn previous_char(b: &[u8], i: usize) -> Option<u8> {
    b[..i]
        .iter()
        .rev()
        .find(|c| **c != b' ' && **c != b'\t')
        .copied()
}

fn vba_tokens(text: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // At the start of a statement (`Rem` is a comment only there).
    let mut statement_start = true;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' | b'\r' => {
                statement_start = true;
                i += 1;
            }
            b' ' | b'\t' => i += 1,
            b':' => {
                statement_start = true;
                i += 1;
            }
            b'\'' => {
                let end = line_end(b, i);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                loop {
                    match b.get(j) {
                        None | Some(b'\n') | Some(b'\r') => break,
                        Some(b'"') if b.get(j + 1) == Some(&b'"') => j += 2,
                        Some(b'"') => {
                            j += 1;
                            break;
                        }
                        Some(_) => j += 1,
                    }
                }
                out.push((i..j, TokenKind::String));
                i = j;
                statement_start = false;
            }
            b'&' if matches!(b.get(i + 1), Some(b'H' | b'h' | b'O' | b'o'))
                && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit) =>
            {
                let mut j = i + 2;
                while j < b.len() && b[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j < b.len() && matches!(b[j], b'&' | b'%' | b'^') {
                    j += 1;
                }
                out.push((i..j, TokenKind::Number));
                i = j;
                statement_start = false;
            }
            c if c.is_ascii_digit()
                || (c == b'.'
                    && b.get(i + 1).is_some_and(u8::is_ascii_digit)
                    && !previous_char(b, i).is_some_and(is_ident)) =>
            {
                let mut j = number_end(b, i);
                if j < b.len() && matches!(b[j], b'#' | b'!' | b'@' | b'%' | b'&' | b'^') {
                    j += 1;
                }
                out.push((i..j, TokenKind::Number));
                i = j;
                statement_start = false;
            }
            c if is_ident_start(c) => {
                let mut j = i;
                while j < b.len() && is_ident(b[j]) {
                    j += 1;
                }
                let word = text[i..j].to_ascii_lowercase();
                let member = previous_char(b, i) == Some(b'.');
                if word == "rem" && statement_start {
                    let end = line_end(b, i);
                    out.push((i..end, TokenKind::Comment));
                    i = end;
                    continue;
                }
                if !member && VBA_KEYWORDS.binary_search(&word.as_str()).is_ok() {
                    out.push((i..j, TokenKind::Keyword));
                }
                i = j;
                statement_start = false;
            }
            _ => {
                i += 1;
                statement_start = false;
            }
        }
    }
    out
}

fn line_end(b: &[u8], from: usize) -> usize {
    b[from..]
        .iter()
        .position(|c| *c == b'\n' || *c == b'\r')
        .map_or(b.len(), |p| from + p)
}

fn js_tokens(text: &str) -> Vec<(Range<usize>, TokenKind)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = line_end(b, i);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..].find("*/").map_or(b.len(), |p| i + 2 + p + 2);
                out.push((i..end, TokenKind::Comment));
                i = end;
            }
            b'"' | b'\'' | b'`' => {
                let mut j = i + 1;
                while j < b.len() {
                    match b[j] {
                        b'\\' => j += 2,
                        b'\n' if c != b'`' => break,
                        q if q == c => {
                            j += 1;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                let j = j.min(b.len());
                out.push((i..j, TokenKind::String));
                i = j;
            }
            c if c.is_ascii_digit() && !previous_char(b, i).is_some_and(is_ident) => {
                let j = if c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
                    let mut j = i + 2;
                    while j < b.len() && b[j].is_ascii_hexdigit() {
                        j += 1;
                    }
                    j
                } else {
                    number_end(b, i)
                };
                out.push((i..j, TokenKind::Number));
                i = j;
            }
            c if is_ident_start(c) || c == b'$' => {
                let mut j = i;
                while j < b.len() && (is_ident(b[j]) || b[j] == b'$') {
                    j += 1;
                }
                let member = previous_char(b, i) == Some(b'.');
                if !member && JS_KEYWORDS.binary_search(&&text[i..j]).is_ok() {
                    out.push((i..j, TokenKind::Keyword));
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

/// The editor's highlighter: the whole text lexed after each change.
struct CodeHighlighter {
    language: CodeLanguage,
    tokens: Vec<(Range<usize>, TokenKind)>,
}

impl InputHighlighter for CodeHighlighter {
    fn language(&self) -> SharedString {
        self.language.name().into()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.tokens = tokens(&text.to_string(), self.language);
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut out = Vec::new();
        let mut at = range.start;
        let first = self.tokens.partition_point(|(r, _)| r.end <= range.start);
        for (r, kind) in &self.tokens[first..] {
            if r.start >= range.end {
                break;
            }
            let start = r.start.max(range.start);
            let end = r.end.min(range.end);
            if start > at {
                out.push((at..start, HighlightStyle::default()));
            }
            if end > start {
                let style = resolver
                    .style(kind.style_name())
                    .unwrap_or_else(|| kind.fallback());
                out.push((start..end, style));
                at = end;
            }
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

/// Makes the editor's highlighters (for the languages above).
pub(crate) fn factory() -> InputHighlighterFactory {
    std::rc::Rc::new(|name: &str| {
        let language = CodeLanguage::from_name(name)?;
        Some(Box::new(CodeHighlighter {
            language,
            tokens: Vec::new(),
        }) as Box<dyn InputHighlighter>)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, language: CodeLanguage) -> Vec<(&str, TokenKind)> {
        tokens(text, language)
            .into_iter()
            .map(|(r, k)| (&text[r], k))
            .collect()
    }

    #[std::prelude::v1::test]
    fn keyword_lists_are_sorted() {
        assert!(VBA_KEYWORDS.windows(2).all(|w| w[0] < w[1]));
        assert!(JS_KEYWORDS.windows(2).all(|w| w[0] < w[1]));
    }

    #[std::prelude::v1::test]
    fn vba() {
        use TokenKind::*;
        let code = "Sub Go() ' start\n    Dim s As String: s = \"a \"\"b\"\" c\"\n    x = &HFF + 1.5E3 + 2#\n    Range(\"A1\").End(xlUp).Select\nRem note\nEnd Sub\n";
        assert_eq!(
            kinds(code, CodeLanguage::Vba),
            vec![
                ("Sub", Keyword),
                ("' start", Comment),
                ("Dim", Keyword),
                ("As", Keyword),
                ("String", Keyword),
                ("\"a \"\"b\"\" c\"", String),
                ("&HFF", Number),
                ("1.5E3", Number),
                ("2#", Number),
                ("\"A1\"", String),
                ("Rem note", Comment),
                ("End", Keyword),
                ("Sub", Keyword),
            ]
        );
        // Digits inside names aren't numbers; an unclosed string ends at
        // the line's end.
        assert_eq!(
            kinds("Sheet1.Range(\"A1\nx = 1", CodeLanguage::Vba),
            vec![("\"A1", String), ("1", Number)]
        );
    }

    #[std::prelude::v1::test]
    fn javascript() {
        use TokenKind::*;
        let code =
            "// hi\nfunction go() {\n  const s = 'it\\'s'; /* a\nb */ return 0x1F + x2;\n}\n";
        assert_eq!(
            kinds(code, CodeLanguage::JavaScript),
            vec![
                ("// hi", Comment),
                ("function", Keyword),
                ("const", Keyword),
                ("'it\\'s'", String),
                ("/* a\nb */", Comment),
                ("return", Keyword),
                ("0x1F", Number),
            ]
        );
    }
}
