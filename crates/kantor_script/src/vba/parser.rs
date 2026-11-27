//! VBA parser: tokens to a [`ModuleDef`] (MS-VBAL §5 "Module bodies", §5.4 statements, §5.6
//! expressions).
//!
//! A simple statement that doesn't parse becomes [`StmtKind::Invalid`], and statements Kantor
//! doesn't run (file I/O, `Load`, `RaiseEvent`…) become [`StmtKind::Unsupported`], so one bad
//! line doesn't block a module; both raise an error only when executed. Broken block structure
//! (`If` without `End If`) fails the module.

use std::rc::Rc;

use super::ast::*;
use super::lexer::{Tok, Token, tokenize};
use super::preprocess::preprocess;
use crate::object::{Location, ScriptError, Value};

type R<T> = Result<T, ScriptError>;

/// Parses a module's source.
pub fn parse_module(src: &str) -> R<ModuleDef> {
    let src = preprocess(src)?;
    let mut p = Parser::new(tokenize(&src)?);
    p.module()
}

/// Parses one expression (used by `#If` and tests).
pub fn parse_expression(src: &str) -> R<Expr> {
    let mut p = Parser::new(tokenize(src)?);
    let e = p.expr()?;
    p.skip_separators();
    if !p.at(&Tok::Eof) {
        return Err(p.err("Expected end of expression"));
    }
    Ok(e)
}

/// Words that can't start an expression.
const RESERVED: &[&str] = &[
    "and", "or", "xor", "eqv", "imp", "mod", "like", "is", "then", "else", "elseif", "to", "step",
    "loop", "next", "wend", "case", "end", "select", "as",
];

pub(crate) struct Parser {
    toks: Vec<Token>,
    pos: usize,
    with_depth: usize,
    pending_next: usize,
    default_member: bool,
    enumerator: bool,
    deftypes: Vec<(char, char, Ty)>,
}

impl Parser {
    pub(crate) fn new(toks: Vec<Token>) -> Parser {
        Parser {
            toks,
            pos: 0,
            with_depth: 0,
            pending_next: 0,
            default_member: false,
            enumerator: false,
            deftypes: Vec::new(),
        }
    }

    // ---- Token helpers ----

    fn tok_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }

    fn tok(&self) -> &Tok {
        self.tok_at(0)
    }

    fn token(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn line(&self) -> u32 {
        self.token().line
    }

    fn bump(&mut self) {
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
    }

    fn at(&self, t: &Tok) -> bool {
        self.tok() == t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.tok(), Tok::Op(o) if *o == op)
    }

    fn word_at(&self, n: usize) -> Option<&str> {
        match self.tok_at(n) {
            Tok::Ident(w, None) => Some(w),
            _ => None,
        }
    }

    fn is_word_at(&self, n: usize, w: &str) -> bool {
        self.word_at(n).is_some_and(|x| x.eq_ignore_ascii_case(w))
    }

    fn is_word(&self, w: &str) -> bool {
        self.is_word_at(0, w)
    }

    fn is_words(&self, ws: &[&str]) -> bool {
        ws.iter().enumerate().all(|(i, w)| self.is_word_at(i, w))
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.is_word(w) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, w: &str) -> R<()> {
        if self.eat_word(w) {
            Ok(())
        } else {
            Err(self.err(format!("Expected '{w}'")))
        }
    }

    fn expect(&mut self, t: &Tok, what: &str) -> R<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(self.err(format!("Expected {what}")))
        }
    }

    fn at_eos(&self) -> bool {
        matches!(self.tok(), Tok::Newline | Tok::Colon | Tok::Eof)
    }

    fn skip_separators(&mut self) {
        while matches!(self.tok(), Tok::Newline | Tok::Colon) {
            self.bump();
        }
    }

    fn skip_to_eos(&mut self) {
        while !self.at_eos() {
            self.bump();
        }
    }

    fn err(&self, msg: impl Into<String>) -> ScriptError {
        let mut e = ScriptError::syntax(msg);
        e.location = Some(Location {
            module: String::new(),
            procedure: String::new(),
            line: self.line(),
        });
        e
    }

    fn deftype(&self, name: &str) -> Ty {
        let Some(c) = name.chars().next().map(|c| c.to_ascii_uppercase()) else {
            return Ty::Variant;
        };
        self.deftypes
            .iter()
            .rev()
            .find(|(a, b, _)| (*a..=*b).contains(&c))
            .map_or(Ty::Variant, |(_, _, t)| t.clone())
    }

    /// A name, with its type-declaration character.
    fn ident(&mut self) -> R<(Ident, Option<char>)> {
        match self.tok().clone() {
            Tok::Ident(name, suffix) => {
                self.bump();
                Ok((Ident::new(&name), suffix))
            }
            Tok::Foreign(name) => {
                self.bump();
                Ok((Ident::new(&name), None))
            }
            _ => Err(self.err("Expected identifier")),
        }
    }

    /// A dotted type or class name, lower case (`Scripting.Dictionary`).
    fn dotted_name(&mut self) -> R<Rc<str>> {
        let (first, _) = self.ident()?;
        let mut name = first.key.to_string();
        while self.at(&Tok::Dot) {
            self.bump();
            let (part, _) = self.ident()?;
            name.push('.');
            name.push_str(&part.key);
        }
        Ok(Rc::from(name))
    }

    fn label_name(&mut self) -> R<Rc<str>> {
        match self.tok().clone() {
            Tok::Ident(name, None) => {
                self.bump();
                Ok(Rc::from(name.to_lowercase()))
            }
            Tok::Lit(Value::Integer(n)) => {
                self.bump();
                Ok(Rc::from(n.to_string()))
            }
            Tok::Lit(Value::Long(n)) => {
                self.bump();
                Ok(Rc::from(n.to_string()))
            }
            _ => Err(self.err("Expected label")),
        }
    }

    // ---- Module level ----

    fn module(&mut self) -> R<ModuleDef> {
        let mut m = ModuleDef::default();
        loop {
            self.skip_separators();
            if self.at(&Tok::Eof) {
                break;
            }
            if self.eat_word("option") {
                if self.eat_word("explicit") {
                    m.option_explicit = true;
                } else if self.eat_word("base") {
                    m.option_base = match self.tok() {
                        Tok::Lit(Value::Integer(1)) => 1,
                        Tok::Lit(Value::Integer(0)) => 0,
                        _ => return Err(self.err("Expected 0 or 1")),
                    };
                    self.bump();
                } else if self.eat_word("compare") {
                    m.option_compare_text = self.is_word("text");
                    self.bump();
                } else if self.eat_word("private") {
                    self.expect_word("module")?;
                    m.option_private = true;
                } else {
                    return Err(self.err("Expected Explicit, Base, Compare or Private Module"));
                }
                self.end_of_statement()?;
                continue;
            }
            if self.is_word("attribute") || self.is_word("implements") {
                self.skip_to_eos();
                continue;
            }
            if let Some(ty) = self.word_at(0).and_then(deftype_word) {
                self.bump();
                self.deftype_ranges(ty)?;
                m.deftypes = self.deftypes.clone();
                continue;
            }
            let mut public: Option<bool> = None;
            let mut is_static = false;
            loop {
                if self.eat_word("public") || self.eat_word("global") || self.eat_word("friend") {
                    public = Some(true);
                } else if self.eat_word("private") {
                    public = Some(false);
                } else if self.is_word("static") && !self.is_word_at(1, "as") {
                    self.bump();
                    is_static = true;
                } else {
                    break;
                }
            }
            let word = self
                .word_at(0)
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            match word.as_str() {
                "sub" | "function" | "property" => {
                    let p = self.procedure(public.unwrap_or(true), is_static)?;
                    m.items.push(ModuleItem::Proc(p));
                }
                "const" => {
                    self.bump();
                    for decl in self.const_list()? {
                        m.items.push(ModuleItem::Const {
                            public: public.unwrap_or(false),
                            decl,
                        });
                    }
                }
                "type" => {
                    self.bump();
                    let t = self.type_def()?;
                    m.items.push(ModuleItem::Type(t));
                }
                "enum" => {
                    self.bump();
                    let e = self.enum_def(public.unwrap_or(true))?;
                    m.items.push(ModuleItem::Enum(e));
                }
                "declare" => {
                    self.bump();
                    self.eat_word("ptrsafe");
                    if !self.eat_word("sub") {
                        self.expect_word("function")?;
                    }
                    let (name, _) = self.ident()?;
                    self.expect_word("lib")?;
                    let lib = match self.tok() {
                        Tok::Lit(Value::String(s)) => s.clone(),
                        _ => String::new(),
                    };
                    self.skip_to_eos();
                    m.items.push(ModuleItem::Declare { name, lib });
                }
                "event" => self.skip_to_eos(),
                _ => {
                    let is_dim = self.eat_word("dim");
                    if public.is_none() && !is_dim {
                        return Err(self.err("Invalid outside procedure"));
                    }
                    for decl in self.decl_list()? {
                        m.items.push(ModuleItem::Var {
                            public: public.unwrap_or(false),
                            decl,
                        });
                    }
                }
            }
            self.end_of_statement()?;
        }
        Ok(m)
    }

    fn end_of_statement(&mut self) -> R<()> {
        if self.at_eos() {
            Ok(())
        } else {
            Err(self.err("Expected end of statement"))
        }
    }

    fn deftype_ranges(&mut self, ty: Ty) -> R<()> {
        loop {
            let (a, _) = self.ident()?;
            let a = a.text.chars().next().unwrap_or('A').to_ascii_uppercase();
            let mut b = a;
            if self.is_op("-") {
                self.bump();
                let (z, _) = self.ident()?;
                b = z.text.chars().next().unwrap_or('Z').to_ascii_uppercase();
            }
            self.deftypes.push((a, b, ty.clone()));
            if !self.eat(&Tok::Comma) {
                return Ok(());
            }
        }
    }

    fn type_ref(&mut self) -> R<Ty> {
        let name = self.dotted_name()?;
        Ok(match &*name {
            "variant" | "any" => Ty::Variant,
            "boolean" => Ty::Boolean,
            "byte" => Ty::Byte,
            "integer" => Ty::Integer,
            "long" => Ty::Long,
            "longlong" | "longptr" => Ty::LongLong,
            "single" => Ty::Single,
            "double" | "decimal" => Ty::Double,
            "currency" => Ty::Currency,
            "date" => Ty::Date,
            "string" => {
                if self.is_op("*") {
                    self.bump();
                    match self.tok().clone() {
                        Tok::Lit(Value::Integer(n)) if n > 0 => {
                            self.bump();
                            Ty::FixedString(n as usize)
                        }
                        Tok::Lit(Value::Long(n)) if n > 0 => {
                            self.bump();
                            Ty::FixedString(n as usize)
                        }
                        _ => {
                            // A constant length: keep it a plain String.
                            self.ident()?;
                            Ty::String
                        }
                    }
                } else {
                    Ty::String
                }
            }
            "object" => Ty::Object,
            _ => Ty::Named(name),
        })
    }

    fn bounds_list(&mut self) -> R<Vec<Bounds>> {
        let mut out = Vec::new();
        loop {
            let a = self.expr()?;
            if self.eat_word("to") {
                out.push(Bounds {
                    lower: Some(a),
                    upper: self.expr()?,
                });
            } else {
                out.push(Bounds {
                    lower: None,
                    upper: a,
                });
            }
            if !self.eat(&Tok::Comma) {
                return Ok(out);
            }
        }
    }

    fn var_decl(&mut self) -> R<VarDecl> {
        self.eat_word("withevents");
        let line = self.line();
        let (name, suffix) = self.ident()?;
        let mut ty = suffix.and_then(Ty::from_suffix);
        let mut array = None;
        if self.eat(&Tok::LParen) {
            if self.eat(&Tok::RParen) {
                array = Some(Vec::new());
            } else {
                array = Some(self.bounds_list()?);
                self.expect(&Tok::RParen, "')'")?;
            }
        }
        let mut auto_new = false;
        if self.eat_word("as") {
            auto_new = self.eat_word("new");
            ty = Some(self.type_ref()?);
        }
        let ty = ty.unwrap_or_else(|| self.deftype(&name.text));
        Ok(VarDecl {
            name,
            ty,
            array,
            auto_new,
            line,
        })
    }

    fn decl_list(&mut self) -> R<Vec<VarDecl>> {
        let mut out = vec![self.var_decl()?];
        while self.eat(&Tok::Comma) {
            out.push(self.var_decl()?);
        }
        Ok(out)
    }

    fn const_list(&mut self) -> R<Vec<ConstDecl>> {
        let mut out = Vec::new();
        loop {
            let line = self.line();
            let (name, suffix) = self.ident()?;
            let mut ty = suffix.and_then(Ty::from_suffix);
            if self.eat_word("as") {
                ty = Some(self.type_ref()?);
            }
            if !self.is_op("=") {
                return Err(self.err("Expected '='"));
            }
            self.bump();
            let value = self.expr()?;
            out.push(ConstDecl {
                name,
                ty: ty.unwrap_or(Ty::Variant),
                value,
                line,
            });
            if !self.eat(&Tok::Comma) {
                return Ok(out);
            }
        }
    }

    fn type_def(&mut self) -> R<TypeDef> {
        let (name, _) = self.ident()?;
        let mut fields = Vec::new();
        loop {
            self.skip_separators();
            if self.at(&Tok::Eof) {
                return Err(self.err("Expected 'End Type'"));
            }
            if self.is_words(&["end", "type"]) {
                self.bump();
                self.bump();
                return Ok(TypeDef { name, fields });
            }
            fields.push(self.var_decl()?);
            self.end_of_statement()?;
        }
    }

    fn enum_def(&mut self, public: bool) -> R<EnumDef> {
        let (name, _) = self.ident()?;
        let mut members = Vec::new();
        loop {
            self.skip_separators();
            if self.at(&Tok::Eof) {
                return Err(self.err("Expected 'End Enum'"));
            }
            if self.is_words(&["end", "enum"]) {
                self.bump();
                self.bump();
                return Ok(EnumDef {
                    name,
                    public,
                    members,
                });
            }
            let (member, _) = self.ident()?;
            let value = if self.is_op("=") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            members.push((member, value));
            self.end_of_statement()?;
        }
    }

    fn procedure(&mut self, public: bool, is_static: bool) -> R<ProcDef> {
        let line = self.line();
        let (kind, end_word) = if self.eat_word("sub") {
            (ProcKind::Sub, "sub")
        } else if self.eat_word("function") {
            (ProcKind::Function, "function")
        } else {
            self.expect_word("property")?;
            let k = if self.eat_word("get") {
                ProcKind::PropertyGet
            } else if self.eat_word("let") {
                ProcKind::PropertyLet
            } else {
                self.expect_word("set")?;
                ProcKind::PropertySet
            };
            (k, "property")
        };
        let (name, suffix) = self.ident()?;
        let params = if self.eat(&Tok::LParen) {
            self.params()?
        } else {
            Vec::new()
        };
        let mut ret = suffix
            .and_then(Ty::from_suffix)
            .unwrap_or_else(|| self.deftype(&name.text));
        if self.eat_word("as") {
            ret = self.type_ref()?;
            if self.at(&Tok::LParen) && self.tok_at(1) == &Tok::RParen {
                self.bump();
                self.bump();
                ret = Ty::Array(Box::new(ret));
            }
        }
        self.end_of_statement()?;
        self.default_member = false;
        self.enumerator = false;
        self.with_depth = 0;
        self.pending_next = 0;
        let body = self.block(&[&["end", end_word]])?;
        self.bump();
        self.bump();
        Ok(ProcDef {
            name,
            kind,
            public,
            is_static,
            params,
            ret,
            body,
            line,
            default_member: self.default_member,
            enumerator: self.enumerator,
        })
    }

    fn params(&mut self) -> R<Vec<Param>> {
        let mut out = Vec::new();
        if self.eat(&Tok::RParen) {
            return Ok(out);
        }
        loop {
            let optional = self.eat_word("optional");
            let byval = if self.eat_word("byval") {
                true
            } else {
                self.eat_word("byref");
                false
            };
            let paramarray = self.eat_word("paramarray");
            let (name, suffix) = self.ident()?;
            let mut is_array = false;
            if self.at(&Tok::LParen) {
                self.bump();
                self.expect(&Tok::RParen, "')'")?;
                is_array = true;
            }
            let mut ty = suffix
                .and_then(Ty::from_suffix)
                .unwrap_or_else(|| self.deftype(&name.text));
            if self.eat_word("as") {
                ty = self.type_ref()?;
            }
            if is_array || paramarray {
                ty = Ty::Array(Box::new(ty));
            }
            let default = if self.is_op("=") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            out.push(Param {
                name,
                ty,
                byval,
                optional: optional || paramarray,
                default,
                paramarray,
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        Ok(out)
    }

    // ---- Blocks ----

    fn at_terms(&self, terms: &[&[&str]]) -> bool {
        terms.iter().any(|t| self.is_words(t))
            || (terms.iter().any(|t| *t == ["end", "if"]) && self.is_word("endif"))
    }

    fn block(&mut self, terms: &[&[&str]]) -> R<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            // `Next j, i` closed this loop too: stop before the line break so the enclosing
            // For statement still ends at the end of that line.
            if self.pending_next > 0 {
                if terms.iter().any(|t| *t == ["next"]) {
                    return Ok(out);
                }
                return Err(self.err("Next without For"));
            }
            self.skip_separators();
            if self.at(&Tok::Eof) {
                return Err(self.err(format!("Expected '{}'", title(terms[terms.len() - 1]))));
            }
            if self.at_terms(terms) {
                return Ok(out);
            }
            for end in ["sub", "function", "property"] {
                if self.is_words(&["end", end]) {
                    return Err(self.err(format!(
                        "Expected '{}' before 'End {}'",
                        title(terms[terms.len() - 1]),
                        title(&[end])
                    )));
                }
            }
            let line = self.line();
            if let Some(label) = self.label() {
                out.push(Stmt {
                    line,
                    kind: StmtKind::Label(label),
                });
                continue;
            }
            out.push(self.statement_recovering()?);
        }
    }

    /// A label at the start of a line: `name:` or a line number.
    fn label(&mut self) -> Option<Rc<str>> {
        if !self.token().line_start {
            return None;
        }
        match self.tok().clone() {
            Tok::Ident(name, None)
                if self.tok_at(1) == &Tok::Colon
                    && !RESERVED.contains(&name.to_ascii_lowercase().as_str()) =>
            {
                self.bump();
                self.bump();
                Some(Rc::from(name.to_lowercase()))
            }
            Tok::Lit(Value::Integer(n)) => {
                self.bump();
                self.eat(&Tok::Colon);
                Some(Rc::from(n.to_string()))
            }
            Tok::Lit(Value::Long(n)) => {
                self.bump();
                self.eat(&Tok::Colon);
                Some(Rc::from(n.to_string()))
            }
            _ => None,
        }
    }

    /// Whether the statement at `pos` opens or closes a block (its errors can't be skipped).
    fn is_structural(&self, pos: usize) -> bool {
        let word = |i: usize| match self.toks.get(i).map(|t| &t.tok) {
            Some(Tok::Ident(w, None)) => w.to_ascii_lowercase(),
            _ => String::new(),
        };
        match word(pos).as_str() {
            "select" | "for" | "do" | "while" | "with" | "next" | "loop" | "wend" | "else"
            | "elseif" | "case" | "endif" => true,
            "end" => matches!(
                word(pos + 1).as_str(),
                "if" | "select" | "with" | "sub" | "function" | "property" | "type" | "enum"
            ),
            "if" => {
                // A block If ends its line right after Then.
                let mut i = pos;
                while let Some(t) = self.toks.get(i) {
                    match &t.tok {
                        Tok::Newline | Tok::Eof => return false,
                        Tok::Ident(w, None) if w.eq_ignore_ascii_case("then") => {
                            return matches!(
                                self.toks.get(i + 1).map(|t| &t.tok),
                                Some(Tok::Newline | Tok::Eof)
                            );
                        }
                        _ => i += 1,
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn statement_recovering(&mut self) -> R<Stmt> {
        let start = self.pos;
        let line = self.line();
        let result = self.statement().and_then(|s| {
            if self.at_eos() {
                Ok(s)
            } else {
                Err(self.err("Expected end of statement"))
            }
        });
        match result {
            Ok(s) => Ok(s),
            Err(e) if !self.is_structural(start) => {
                self.pos = start;
                self.skip_to_eos();
                Ok(Stmt {
                    line,
                    kind: StmtKind::Invalid(e.description),
                })
            }
            Err(e) => Err(e),
        }
    }

    // ---- Statements ----

    fn statement(&mut self) -> R<Stmt> {
        let line = self.line();
        let kind = self.statement_kind()?;
        Ok(Stmt { line, kind })
    }

    fn unsupported(&mut self, feature: &str) -> R<StmtKind> {
        self.skip_to_eos();
        Ok(StmtKind::Unsupported(feature.to_string()))
    }

    fn statement_kind(&mut self) -> R<StmtKind> {
        let word = self
            .word_at(0)
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let next = self.tok_at(1).clone();
        let next_is_hash = next == Tok::Hash;
        match word.as_str() {
            "dim" => {
                self.bump();
                return Ok(StmtKind::Dim {
                    decls: self.decl_list()?,
                    is_static: false,
                });
            }
            "static" => {
                self.bump();
                return Ok(StmtKind::Dim {
                    decls: self.decl_list()?,
                    is_static: true,
                });
            }
            "const" => {
                self.bump();
                return Ok(StmtKind::Const(self.const_list()?));
            }
            "redim" => return self.redim(),
            "erase" => {
                self.bump();
                let mut targets = vec![self.postfix(false)?];
                while self.eat(&Tok::Comma) {
                    targets.push(self.postfix(false)?);
                }
                return Ok(StmtKind::Erase(targets));
            }
            "set" => {
                self.bump();
                return self.assignment(true);
            }
            "let" => {
                self.bump();
                return self.assignment(false);
            }
            "call" => {
                self.bump();
                let target = self.postfix(false)?;
                return Ok(match target {
                    Expr::Call(t, args) => StmtKind::Call { target: *t, args },
                    t => StmtKind::Call {
                        target: t,
                        args: Vec::new(),
                    },
                });
            }
            "if" => return self.if_stmt(),
            "select" if self.is_word_at(1, "case") => return self.select_stmt(),
            "for" => return self.for_stmt(),
            "do" => return self.do_stmt(),
            "while" => {
                self.bump();
                let cond = self.expr()?;
                let body = self.block(&[&["wend"]])?;
                self.bump();
                return Ok(StmtKind::Do {
                    kind: DoKind::PreWhile,
                    cond: Some(cond),
                    body,
                });
            }
            "with" => {
                self.bump();
                let object = self.expr()?;
                let depth = self.with_depth;
                self.with_depth += 1;
                let body = self.block(&[&["end", "with"]]);
                self.with_depth -= 1;
                let body = body?;
                self.bump();
                self.bump();
                return Ok(StmtKind::With {
                    object,
                    depth,
                    body,
                });
            }
            "goto" => {
                self.bump();
                return Ok(StmtKind::GoTo(self.label_name()?));
            }
            "gosub" => {
                self.bump();
                return Ok(StmtKind::GoSub(self.label_name()?));
            }
            "return" if self.tok_at(1) != &Tok::Op("=") => {
                self.bump();
                return Ok(StmtKind::Return);
            }
            "on" => return self.on_stmt(),
            "resume" => {
                self.bump();
                if self.at_eos() || self.is_word("else") {
                    return Ok(StmtKind::Resume(Resume::Here));
                }
                if self.eat_word("next") {
                    return Ok(StmtKind::Resume(Resume::Next));
                }
                if matches!(self.tok(), Tok::Lit(Value::Integer(0))) {
                    self.bump();
                    return Ok(StmtKind::Resume(Resume::Here));
                }
                return Ok(StmtKind::Resume(Resume::Label(self.label_name()?)));
            }
            "exit" => {
                self.bump();
                let kind = if self.eat_word("for") {
                    ExitKind::For
                } else if self.eat_word("do") {
                    ExitKind::Do
                } else if self.eat_word("sub")
                    || self.eat_word("function")
                    || self.eat_word("property")
                {
                    ExitKind::Proc
                } else {
                    return Err(self.err("Expected For, Do, Sub, Function or Property"));
                };
                return Ok(StmtKind::Exit(kind));
            }
            "end" => {
                if matches!(next, Tok::Newline | Tok::Colon | Tok::Eof) {
                    self.bump();
                    return Ok(StmtKind::End);
                }
                if let Tok::Ident(w, None) = &next {
                    return Err(self.err(format!("Unexpected 'End {w}'")));
                }
            }
            "stop" if self.tok_at(1) != &Tok::Op("=") => {
                self.bump();
                return Ok(StmtKind::Stop);
            }
            "debug" if next == Tok::Dot => {
                if self.is_word_at(2, "print") {
                    self.bump();
                    self.bump();
                    self.bump();
                    return Ok(StmtKind::Print(self.print_items()?));
                }
                if self.is_word_at(2, "assert") {
                    self.bump();
                    self.bump();
                    self.bump();
                    return Ok(StmtKind::Assert(self.expr()?));
                }
            }
            "lset" | "rset" => {
                self.bump();
                let target = self.postfix(false)?;
                if !self.is_op("=") {
                    return Err(self.err("Expected '='"));
                }
                self.bump();
                return Ok(StmtKind::Pad {
                    right: word == "rset",
                    target,
                    value: self.expr()?,
                });
            }
            "error"
                if !matches!(next, Tok::LParen | Tok::Op("=") | Tok::Dot) && !self.at_eos_at(1) =>
            {
                self.bump();
                return Ok(StmtKind::Error(self.expr()?));
            }
            "open"
                if !matches!(next, Tok::Op("=") | Tok::Dot | Tok::LParen) && !self.at_eos_at(1) =>
            {
                return self.unsupported("File I/O (the Open statement)");
            }
            "close" | "reset"
                if next_is_hash || self.at_eos_at(1) || matches!(next, Tok::Lit(_)) =>
            {
                return self.unsupported("File I/O (the Close statement)");
            }
            "print" | "write" | "input" | "get" | "put" | "seek" | "lock" | "unlock" | "width"
                if next_is_hash =>
            {
                let feature = format!("File I/O ({} #)", title(&[word.as_str()]));
                return self.unsupported(&feature);
            }
            "line" if self.is_word_at(1, "input") => {
                return self.unsupported("File I/O (Line Input #)");
            }
            "name" if self.line_has_word("as") => {
                return self.unsupported("Renaming files (Name ... As)");
            }
            "load" | "unload" if !matches!(next, Tok::Op("=") | Tok::Dot | Tok::LParen) => {
                return self.unsupported("UserForms (Load/Unload)");
            }
            "raiseevent" => return self.unsupported("Events (RaiseEvent)"),
            "attribute" => {
                // `Attribute Item.VB_UserMemId = 0` marks the class's default member.
                let mut saw = false;
                let mut negative = false;
                while !self.at_eos() {
                    match self.tok() {
                        Tok::Ident(w, _) if w.eq_ignore_ascii_case("vb_usermemid") => saw = true,
                        Tok::Op("-") => negative = true,
                        Tok::Lit(Value::Integer(0)) if saw => self.default_member = true,
                        Tok::Lit(Value::Integer(4)) if saw && negative => self.enumerator = true,
                        _ => {}
                    }
                    self.bump();
                }
                return Ok(StmtKind::Nop);
            }
            "option" | "sub" | "function" | "property" | "type" | "enum" | "declare" | "public"
            | "private" | "global" | "friend" => {
                return Err(self.err(format!(
                    "'{}' is not valid inside a procedure",
                    title(&[word.as_str()])
                )));
            }
            _ => {}
        }
        self.expression_statement()
    }

    fn at_eos_at(&self, n: usize) -> bool {
        matches!(self.tok_at(n), Tok::Newline | Tok::Colon | Tok::Eof)
    }

    fn line_has_word(&self, w: &str) -> bool {
        let mut i = 1;
        while !self.at_eos_at(i) {
            if self.is_word_at(i, w) {
                return true;
            }
            i += 1;
        }
        false
    }

    fn assignment(&mut self, set: bool) -> R<StmtKind> {
        let target = self.postfix(false)?;
        if !self.is_op("=") {
            return Err(self.err("Expected '='"));
        }
        self.bump();
        Ok(StmtKind::Assign {
            set,
            target,
            value: self.expr()?,
        })
    }

    fn expression_statement(&mut self) -> R<StmtKind> {
        if !matches!(
            self.tok(),
            Tok::Ident(..) | Tok::Foreign(_) | Tok::Dot | Tok::Bang
        ) {
            return Err(self.err("Expected statement"));
        }
        let target = self.postfix(true)?;
        if matches!(
            target,
            Expr::Lit(_) | Expr::Paren(_) | Expr::New(_) | Expr::TypeOf(..) | Expr::Unary(..)
        ) {
            return Err(self.err("Expected statement"));
        }
        if self.is_op("=") {
            self.bump();
            let value = self.expr()?;
            if let Expr::Call(f, args) = &target
                && let Expr::Name(id) = &**f
                && matches!(&*id.key, "mid" | "midb")
                && (2..=3).contains(&args.len())
            {
                let arg = |i: usize| args.get(i).and_then(|a| a.value.clone());
                return Ok(StmtKind::MidAssign {
                    target: arg(0).ok_or_else(|| self.err("Expected variable"))?,
                    start: arg(1).ok_or_else(|| self.err("Expected start"))?,
                    length: arg(2),
                    value,
                });
            }
            return Ok(StmtKind::Assign {
                set: false,
                target,
                value,
            });
        }
        let args = if self.at_eos() || self.is_word("else") {
            Vec::new()
        } else {
            self.bare_args()?
        };
        Ok(StmtKind::Call { target, args })
    }

    fn redim(&mut self) -> R<StmtKind> {
        self.bump();
        let preserve = self.eat_word("preserve");
        let mut items = Vec::new();
        loop {
            let mut target = match self.tok().clone() {
                Tok::Dot => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    self.with_member(name)?
                }
                _ => {
                    let (name, _) = self.ident()?;
                    Expr::Name(name)
                }
            };
            while self.at(&Tok::Dot) {
                self.bump();
                let (name, _) = self.ident()?;
                target = Expr::Member(Box::new(target), name);
            }
            self.expect(&Tok::LParen, "'('")?;
            let bounds = self.bounds_list()?;
            self.expect(&Tok::RParen, "')'")?;
            let ty = if self.eat_word("as") {
                Some(self.type_ref()?)
            } else {
                None
            };
            items.push(ReDimItem { target, bounds, ty });
            if !self.eat(&Tok::Comma) {
                return Ok(StmtKind::ReDim { preserve, items });
            }
        }
    }

    fn print_items(&mut self) -> R<Vec<PrintItem>> {
        let mut items = Vec::new();
        while !self.at_eos() && !self.is_word("else") {
            if self.eat(&Tok::Semi) {
                items.push(PrintItem::Semi);
            } else if self.eat(&Tok::Comma) {
                items.push(PrintItem::Comma);
            } else if self.is_word("spc") && self.tok_at(1) == &Tok::LParen {
                self.bump();
                self.bump();
                let e = self.expr()?;
                self.expect(&Tok::RParen, "')'")?;
                items.push(PrintItem::Spc(e));
            } else if self.is_word("tab") {
                self.bump();
                if self.eat(&Tok::LParen) {
                    let e = self.expr()?;
                    self.expect(&Tok::RParen, "')'")?;
                    items.push(PrintItem::Tab(Some(e)));
                } else {
                    items.push(PrintItem::Tab(None));
                }
            } else {
                items.push(PrintItem::Expr(self.expr()?));
            }
        }
        Ok(items)
    }

    fn on_stmt(&mut self) -> R<StmtKind> {
        self.bump();
        self.eat_word("local");
        if !self.eat_word("error") {
            return self.unsupported("Computed branches (On ... GoTo/GoSub)");
        }
        if self.eat_word("resume") {
            self.expect_word("next")?;
            return Ok(StmtKind::OnError(OnError::ResumeNext));
        }
        self.expect_word("goto")?;
        if matches!(self.tok(), Tok::Lit(Value::Integer(0))) {
            self.bump();
            return Ok(StmtKind::OnError(OnError::Disable));
        }
        if self.is_op("-") && matches!(self.tok_at(1), Tok::Lit(Value::Integer(1))) {
            self.bump();
            self.bump();
            return Ok(StmtKind::OnError(OnError::Reset));
        }
        Ok(StmtKind::OnError(OnError::GoTo(self.label_name()?)))
    }

    fn if_stmt(&mut self) -> R<StmtKind> {
        self.bump();
        let cond = self.expr()?;
        self.expect_word("then")?;
        if matches!(self.tok(), Tok::Newline | Tok::Eof) {
            let terms: &[&[&str]] = &[&["elseif"], &["else"], &["end", "if"]];
            let mut arms = vec![IfArm {
                cond,
                body: self.block(terms)?,
            }];
            let mut otherwise = None;
            loop {
                if self.eat_word("elseif") {
                    let cond = self.expr()?;
                    self.expect_word("then")?;
                    let body = self.block(terms)?;
                    arms.push(IfArm { cond, body });
                } else if self.eat_word("else") {
                    otherwise = Some(self.block(&[&["end", "if"]])?);
                } else {
                    if !self.eat_word("endif") {
                        self.expect_word("end")?;
                        self.expect_word("if")?;
                    }
                    break;
                }
            }
            return Ok(StmtKind::If { arms, otherwise });
        }
        let body = self.inline_statements()?;
        let otherwise = if self.eat_word("else") {
            Some(self.inline_statements()?)
        } else {
            None
        };
        Ok(StmtKind::If {
            arms: vec![IfArm { cond, body }],
            otherwise,
        })
    }

    /// The statements of a single-line `If`, up to `Else` or the end of the line.
    fn inline_statements(&mut self) -> R<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            if matches!(self.tok(), Tok::Newline | Tok::Eof) || self.is_word("else") {
                return Ok(out);
            }
            if self.eat(&Tok::Colon) {
                continue;
            }
            let line = self.line();
            if out.is_empty()
                && let Tok::Lit(Value::Integer(_) | Value::Long(_)) = self.tok()
            {
                let label = self.label_name()?;
                out.push(Stmt {
                    line,
                    kind: StmtKind::GoTo(label),
                });
                continue;
            }
            out.push(self.statement()?);
            if !self.at_eos() && !self.is_word("else") {
                return Err(self.err("Expected end of statement"));
            }
        }
    }

    fn select_stmt(&mut self) -> R<StmtKind> {
        self.bump();
        self.bump();
        let test = self.expr()?;
        let mut arms = Vec::new();
        let mut otherwise = None;
        loop {
            self.skip_separators();
            if self.is_words(&["end", "select"]) {
                self.bump();
                self.bump();
                return Ok(StmtKind::Select {
                    test,
                    arms,
                    otherwise,
                });
            }
            self.expect_word("case")?;
            if self.eat_word("else") {
                otherwise = Some(self.block(&[&["end", "select"]])?);
                continue;
            }
            let mut clauses = Vec::new();
            loop {
                if self.eat_word("is") {
                    let op = match self.tok() {
                        Tok::Op("=") => BinOp::Eq,
                        Tok::Op("<>") => BinOp::Ne,
                        Tok::Op("<") => BinOp::Lt,
                        Tok::Op(">") => BinOp::Gt,
                        Tok::Op("<=") => BinOp::Le,
                        Tok::Op(">=") => BinOp::Ge,
                        _ => return Err(self.err("Expected comparison operator")),
                    };
                    self.bump();
                    clauses.push(CaseClause::Is(op, self.expr()?));
                } else {
                    let a = self.expr()?;
                    if self.eat_word("to") {
                        clauses.push(CaseClause::Range(a, self.expr()?));
                    } else {
                        clauses.push(CaseClause::Value(a));
                    }
                }
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            let body = self.block(&[&["case"], &["end", "select"]])?;
            arms.push(CaseArm { clauses, body });
        }
    }

    fn finish_next(&mut self) -> R<()> {
        if self.pending_next > 0 {
            self.pending_next -= 1;
            return Ok(());
        }
        self.expect_word("next")?;
        if !self.at_eos() {
            self.postfix(false)?;
            while self.eat(&Tok::Comma) {
                self.postfix(false)?;
                self.pending_next += 1;
            }
        }
        Ok(())
    }

    fn for_stmt(&mut self) -> R<StmtKind> {
        self.bump();
        if self.eat_word("each") {
            let var = self.postfix(false)?;
            self.expect_word("in")?;
            let collection = self.expr()?;
            let body = self.block(&[&["next"]])?;
            self.finish_next()?;
            return Ok(StmtKind::ForEach {
                var,
                collection,
                body,
            });
        }
        let var = self.postfix(false)?;
        if !self.is_op("=") {
            return Err(self.err("Expected '='"));
        }
        self.bump();
        let from = self.expr()?;
        self.expect_word("to")?;
        let to = self.expr()?;
        let step = if self.eat_word("step") {
            Some(self.expr()?)
        } else {
            None
        };
        let body = self.block(&[&["next"]])?;
        self.finish_next()?;
        Ok(StmtKind::For {
            var,
            from,
            to,
            step,
            body,
        })
    }

    fn do_stmt(&mut self) -> R<StmtKind> {
        self.bump();
        let (mut kind, mut cond) = if self.eat_word("while") {
            (DoKind::PreWhile, Some(self.expr()?))
        } else if self.eat_word("until") {
            (DoKind::PreUntil, Some(self.expr()?))
        } else {
            (DoKind::Forever, None)
        };
        let body = self.block(&[&["loop"]])?;
        self.bump();
        if kind == DoKind::Forever {
            if self.eat_word("while") {
                kind = DoKind::PostWhile;
                cond = Some(self.expr()?);
            } else if self.eat_word("until") {
                kind = DoKind::PostUntil;
                cond = Some(self.expr()?);
            }
        }
        Ok(StmtKind::Do { kind, cond, body })
    }

    // ---- Arguments ----

    fn omitted() -> ArgExpr {
        ArgExpr {
            name: None,
            value: None,
        }
    }

    fn arg(&mut self) -> R<ArgExpr> {
        if matches!(self.tok(), Tok::Ident(..) | Tok::Foreign(_)) && self.tok_at(1) == &Tok::Assign
        {
            let (name, _) = self.ident()?;
            self.bump();
            return Ok(ArgExpr {
                name: Some(name),
                value: Some(self.expr()?),
            });
        }
        Ok(ArgExpr {
            name: None,
            value: Some(self.expr()?),
        })
    }

    fn arg_or_omitted(&mut self) -> R<ArgExpr> {
        if matches!(self.tok(), Tok::Comma | Tok::RParen) || self.at_eos() || self.is_word("else") {
            Ok(Self::omitted())
        } else {
            self.arg()
        }
    }

    /// Arguments of a call statement without parentheses.
    fn bare_args(&mut self) -> R<Vec<ArgExpr>> {
        let mut args = vec![self.arg_or_omitted()?];
        while self.eat(&Tok::Comma) {
            args.push(self.arg_or_omitted()?);
        }
        Ok(args)
    }

    /// Arguments after `(`, through `)`.
    fn paren_args(&mut self) -> R<Vec<ArgExpr>> {
        if self.eat(&Tok::RParen) {
            return Ok(Vec::new());
        }
        let mut args = vec![self.arg_or_omitted()?];
        while self.eat(&Tok::Comma) {
            args.push(self.arg_or_omitted()?);
        }
        self.expect(&Tok::RParen, "')'")?;
        Ok(args)
    }

    // ---- Expressions (MS-VBAL §5.6.9 operator precedence) ----

    pub(crate) fn expr(&mut self) -> R<Expr> {
        self.binary_level(0)
    }

    fn binary_level(&mut self, level: usize) -> R<Expr> {
        // Lowest to highest: Imp, Eqv, Xor, Or, And; then Not; then comparisons.
        const LOGIC: [(&str, BinOp); 5] = [
            ("imp", BinOp::Imp),
            ("eqv", BinOp::Eqv),
            ("xor", BinOp::Xor),
            ("or", BinOp::Or),
            ("and", BinOp::And),
        ];
        if level == LOGIC.len() {
            return self.not_expr();
        }
        let (word, op) = LOGIC[level];
        let mut left = self.binary_level(level + 1)?;
        while self.is_word(word) {
            self.bump();
            let right = self.binary_level(level + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not_expr(&mut self) -> R<Expr> {
        if self.eat_word("not") {
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.not_expr()?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> R<Expr> {
        let mut left = self.concat()?;
        loop {
            let op = match self.tok() {
                Tok::Op("=") => BinOp::Eq,
                Tok::Op("<>") => BinOp::Ne,
                Tok::Op("<") => BinOp::Lt,
                Tok::Op(">") => BinOp::Gt,
                Tok::Op("<=") => BinOp::Le,
                Tok::Op(">=") => BinOp::Ge,
                Tok::Ident(w, None) if w.eq_ignore_ascii_case("like") => BinOp::Like,
                Tok::Ident(w, None) if w.eq_ignore_ascii_case("is") => BinOp::Is,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.concat()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn concat(&mut self) -> R<Expr> {
        let mut left = self.additive()?;
        while self.is_op("&") {
            self.bump();
            let right = self.additive()?;
            left = Expr::Binary(BinOp::Concat, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn additive(&mut self) -> R<Expr> {
        let mut left = self.modulo()?;
        loop {
            let op = if self.is_op("+") {
                BinOp::Add
            } else if self.is_op("-") {
                BinOp::Sub
            } else {
                return Ok(left);
            };
            self.bump();
            let right = self.modulo()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn modulo(&mut self) -> R<Expr> {
        let mut left = self.int_div()?;
        while self.is_word("mod") {
            self.bump();
            let right = self.int_div()?;
            left = Expr::Binary(BinOp::Mod, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn int_div(&mut self) -> R<Expr> {
        let mut left = self.multiplicative()?;
        while self.is_op("\\") {
            self.bump();
            let right = self.multiplicative()?;
            left = Expr::Binary(BinOp::IntDiv, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> R<Expr> {
        let mut left = self.unary()?;
        loop {
            let op = if self.is_op("*") {
                BinOp::Mul
            } else if self.is_op("/") {
                BinOp::Div
            } else {
                return Ok(left);
            };
            self.bump();
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    /// Unary minus binds looser than `^`: `-2 ^ 2` is -4.
    fn unary(&mut self) -> R<Expr> {
        if self.is_op("-") {
            self.bump();
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)));
        }
        if self.is_op("+") {
            self.bump();
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> R<Expr> {
        let mut left = self.postfix(false)?;
        while self.is_op("^") {
            self.bump();
            let right = self.power_operand()?;
            left = Expr::Binary(BinOp::Pow, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn power_operand(&mut self) -> R<Expr> {
        if self.is_op("-") {
            self.bump();
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.power_operand()?)));
        }
        if self.is_op("+") {
            self.bump();
            return self.power_operand();
        }
        self.postfix(false)
    }

    fn with_member(&self, name: Ident) -> R<Expr> {
        if self.with_depth == 0 {
            return Err(self.err("Invalid or unqualified reference"));
        }
        Ok(Expr::Member(
            Box::new(Expr::With(self.with_depth - 1)),
            name,
        ))
    }

    /// A primary expression and its `.member`, `!key` and `(args)` suffixes. In a call
    /// statement (`stmt`), a `(` after a space starts the first argument instead
    /// (`MsgBox (x) & y`).
    pub(crate) fn postfix(&mut self, stmt: bool) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let space = self.token().space_before;
            match self.tok() {
                Tok::Dot if !space => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    e = Expr::Member(Box::new(e), name);
                }
                Tok::Bang if !space => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    e = Expr::Bang(Box::new(e), name);
                }
                Tok::LParen if !(stmt && space) => {
                    self.bump();
                    let args = self.paren_args()?;
                    e = Expr::Call(Box::new(e), args);
                }
                _ => return Ok(e),
            }
        }
    }

    fn primary(&mut self) -> R<Expr> {
        match self.tok().clone() {
            Tok::Lit(v) => {
                self.bump();
                Ok(Expr::Lit(v))
            }
            Tok::LParen => {
                self.bump();
                let e = self.expr()?;
                self.expect(&Tok::RParen, "')'")?;
                Ok(Expr::Paren(Box::new(e)))
            }
            Tok::Dot => {
                self.bump();
                let (name, _) = self.ident()?;
                self.with_member(name)
            }
            Tok::Bang => {
                self.bump();
                let (name, _) = self.ident()?;
                if self.with_depth == 0 {
                    return Err(self.err("Invalid or unqualified reference"));
                }
                Ok(Expr::Bang(Box::new(Expr::With(self.with_depth - 1)), name))
            }
            Tok::Foreign(name) => {
                self.bump();
                Ok(Expr::Evaluate(name))
            }
            Tok::Ident(name, suffix) => {
                let lower = name.to_ascii_lowercase();
                if suffix.is_none() {
                    match lower.as_str() {
                        "true" => {
                            self.bump();
                            return Ok(Expr::Lit(Value::Boolean(true)));
                        }
                        "false" => {
                            self.bump();
                            return Ok(Expr::Lit(Value::Boolean(false)));
                        }
                        "nothing" => {
                            self.bump();
                            return Ok(Expr::Lit(Value::Object(None)));
                        }
                        "empty" => {
                            self.bump();
                            return Ok(Expr::Lit(Value::Empty));
                        }
                        "null" => {
                            self.bump();
                            return Ok(Expr::Lit(Value::Null));
                        }
                        "me" => {
                            self.bump();
                            return Ok(Expr::Me);
                        }
                        "new" => {
                            self.bump();
                            return Ok(Expr::New(self.dotted_name()?));
                        }
                        "typeof" => {
                            self.bump();
                            let e = self.postfix(false)?;
                            self.expect_word("is")?;
                            return Ok(Expr::TypeOf(Box::new(e), self.dotted_name()?));
                        }
                        "not" => {
                            self.bump();
                            return Ok(Expr::Unary(UnOp::Not, Box::new(self.not_expr()?)));
                        }
                        "addressof" => return Err(self.err("AddressOf is not supported")),
                        w if RESERVED.contains(&w) => {
                            return Err(self.err(format!("Unexpected '{name}'")));
                        }
                        _ => {}
                    }
                }
                self.bump();
                Ok(Expr::Name(Ident::new(&name)))
            }
            Tok::Op("-") => {
                self.bump();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)))
            }
            _ => Err(self.err("Expected expression")),
        }
    }
}

fn deftype_word(w: &str) -> Option<Ty> {
    Some(match w.to_ascii_lowercase().as_str() {
        "defbool" => Ty::Boolean,
        "defbyte" => Ty::Byte,
        "defint" => Ty::Integer,
        "deflng" => Ty::Long,
        "deflnglng" | "deflngptr" => Ty::LongLong,
        "defcur" => Ty::Currency,
        "defsng" => Ty::Single,
        "defdbl" => Ty::Double,
        "defdec" => Ty::Double,
        "defdate" => Ty::Date,
        "defstr" => Ty::String,
        "defobj" => Ty::Object,
        "defvar" => Ty::Variant,
        _ => return None,
    })
}

fn title(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence() {
        // MS-VBAL §5.6.9: ^ binds tighter than unary minus; Not is below comparisons.
        let e = parse_expression("-2 ^ 2").unwrap();
        assert!(matches!(e, Expr::Unary(UnOp::Neg, _)));
        let e = parse_expression("Not a = b And c").unwrap();
        let Expr::Binary(BinOp::And, l, _) = e else {
            panic!("expected And at the top")
        };
        assert!(matches!(*l, Expr::Unary(UnOp::Not, _)));
        let e = parse_expression("a & b + c").unwrap();
        assert!(matches!(e, Expr::Binary(BinOp::Concat, _, _)));
        let e = parse_expression("a Mod b \\ c").unwrap();
        assert!(matches!(e, Expr::Binary(BinOp::Mod, _, _)));
    }

    #[test]
    fn statements_and_recovery() {
        let src = "Sub Main()\r\n  MsgBox (\"a\") & \"b\"\r\n  Open \"x\" For Input As #1\r\n  x = = 2\r\n  If a Then b = 1 Else b = 2\r\nEnd Sub\r\n";
        let m = parse_module(src).unwrap();
        let ModuleItem::Proc(p) = &m.items[0] else {
            panic!("expected a procedure")
        };
        assert!(matches!(&p.body[0].kind, StmtKind::Call { args, .. } if args.len() == 1));
        assert!(matches!(&p.body[1].kind, StmtKind::Unsupported(_)));
        assert!(matches!(&p.body[2].kind, StmtKind::Invalid(_)));
        assert_eq!(p.body[2].line, 4);
        assert!(matches!(
            &p.body[3].kind,
            StmtKind::If {
                otherwise: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn missing_end_if_fails_the_module() {
        let err = parse_module("Sub A()\nIf x Then\ny = 1\nEnd Sub\n").unwrap_err();
        assert!(err.description.contains("End If"), "{}", err.description);
    }
}
