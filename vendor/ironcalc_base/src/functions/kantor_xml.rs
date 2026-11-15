//! KANTOR PATCH (functions): FILTERXML.
//!
//! `FILTERXML(xml, xpath)` returns the values of the nodes an XPath 1.0
//! expression selects: one value, or a column of them (spilled). Text that
//! reads as a number is a number. No match, malformed XML, an expression
//! that isn't a node-set, or a namespace prefix (Excel has no way to declare
//! one) give #VALUE!. As in Excel, an unprefixed name matches only elements
//! in no namespace.
//!
//! The XPath subset: location paths with `/` and `//`, the axes (child,
//! descendant, descendant-or-self, self, parent, ancestor,
//! ancestor-or-self, following-sibling, preceding-sibling, following,
//! preceding, attribute) and their abbreviations (`@`, `.`, `..`), name
//! tests and `*`, `text()`, `node()`, `comment()`, predicates (positions
//! too), unions, `or`/`and`, comparisons, arithmetic, and the functions
//! last, position, count, name, local-name, string, concat, starts-with,
//! contains, substring, substring-before, substring-after, string-length,
//! normalize-space, translate, not, true, false, boolean, number, sum,
//! floor, ceiling and round.

use roxmltree::{Document, Node as XmlNode, NodeType};

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{ArrayNode, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    model::Model,
};

// ── Tokens ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Slash,
    DoubleSlash,
    LBracket,
    RBracket,
    LParen,
    RParen,
    At,
    Comma,
    Dot,
    DotDot,
    DoubleColon,
    Pipe,
    Plus,
    Minus,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Star,
    And,
    Or,
    Div,
    Mod,
    /// A name test (`name`, `*`, `prefix:*`, `prefix:name`).
    Name(String),
    Literal(String),
    Number(f64),
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\u{b7}')
}

fn tokenize(text: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    // XPath's rule: after a token that isn't an operator, `@`, `::`, `(`,
    // `[` or `,`, a `*` multiplies and a name is an operator name.
    let operator_context = |out: &Vec<Tok>| match out.last() {
        None => false,
        Some(t) => !matches!(
            t,
            Tok::At
                | Tok::DoubleColon
                | Tok::LParen
                | Tok::LBracket
                | Tok::Comma
                | Tok::Slash
                | Tok::DoubleSlash
                | Tok::Pipe
                | Tok::Plus
                | Tok::Minus
                | Tok::Eq
                | Tok::Ne
                | Tok::Lt
                | Tok::Le
                | Tok::Gt
                | Tok::Ge
                | Tok::Star
                | Tok::And
                | Tok::Or
                | Tok::Div
                | Tok::Mod
        ),
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                i += 1;
                continue;
            }
            '/' if next == Some('/') => {
                out.push(Tok::DoubleSlash);
                i += 2;
            }
            '/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '@' => {
                out.push(Tok::At);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            ':' if next == Some(':') => {
                out.push(Tok::DoubleColon);
                i += 2;
            }
            '|' => {
                out.push(Tok::Pipe);
                i += 1;
            }
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            '!' if next == Some('=') => {
                out.push(Tok::Ne);
                i += 2;
            }
            '<' if next == Some('=') => {
                out.push(Tok::Le);
                i += 2;
            }
            '<' => {
                out.push(Tok::Lt);
                i += 1;
            }
            '>' if next == Some('=') => {
                out.push(Tok::Ge);
                i += 2;
            }
            '>' => {
                out.push(Tok::Gt);
                i += 1;
            }
            '"' | '\'' => {
                let end = chars[i + 1..].iter().position(|&d| d == c)? + i + 1;
                out.push(Tok::Literal(chars[i + 1..end].iter().collect()));
                i = end + 1;
            }
            '.' if next == Some('.') => {
                out.push(Tok::DotDot);
                i += 2;
            }
            '.' if !next.is_some_and(|d| d.is_ascii_digit()) => {
                out.push(Tok::Dot);
                i += 1;
            }
            '*' => {
                if operator_context(&out) {
                    out.push(Tok::Star);
                } else {
                    out.push(Tok::Name("*".to_string()));
                }
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                out.push(Tok::Number(s.parse().ok()?));
            }
            c if is_name_start(c) => {
                let start = i;
                while i < chars.len() && is_name_char(chars[i]) {
                    i += 1;
                }
                // prefix:name or prefix:* (not the `::` of an axis).
                if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] != ':' {
                    i += 1;
                    if chars[i] == '*' {
                        i += 1;
                    } else {
                        while i < chars.len() && is_name_char(chars[i]) {
                            i += 1;
                        }
                    }
                }
                let name: String = chars[start..i].iter().collect();
                if operator_context(&out) {
                    out.push(match name.as_str() {
                        "and" => Tok::And,
                        "or" => Tok::Or,
                        "div" => Tok::Div,
                        "mod" => Tok::Mod,
                        _ => return None,
                    });
                } else {
                    out.push(Tok::Name(name));
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

// ── Syntax ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    SelfAxis,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Following,
    Preceding,
    Attribute,
}

#[derive(Clone, Debug)]
enum NodeTest {
    /// A name, or `*` (None).
    Name(Option<String>),
    Text,
    Node,
    Comment,
}

#[derive(Clone, Debug)]
struct Step {
    axis: Axis,
    test: NodeTest,
    predicates: Vec<Expr>,
}

#[derive(Clone, Debug)]
enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Compare(Tok, Box<Expr>, Box<Expr>),
    Arith(Tok, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Union(Box<Expr>, Box<Expr>),
    Literal(String),
    Number(f64),
    Call(String, Vec<Expr>),
    /// A path: absolute or not, from an optional filter expression.
    Path {
        start: Option<Box<Expr>>,
        absolute: bool,
        steps: Vec<Step>,
    },
    Filter(Box<Expr>, Vec<Expr>),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

type Parse<T> = Result<T, ()>;

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok) -> Parse<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn expr(&mut self) -> Parse<Expr> {
        let mut left = self.and_expr()?;
        while self.eat(&Tok::Or) {
            left = Expr::Or(Box::new(left), Box::new(self.and_expr()?));
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Parse<Expr> {
        let mut left = self.equality()?;
        while self.eat(&Tok::And) {
            left = Expr::And(Box::new(left), Box::new(self.equality()?));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Parse<Expr> {
        let mut left = self.relational()?;
        while let Some(t @ (Tok::Eq | Tok::Ne)) = self.peek().cloned() {
            self.pos += 1;
            left = Expr::Compare(t, Box::new(left), Box::new(self.relational()?));
        }
        Ok(left)
    }

    fn relational(&mut self) -> Parse<Expr> {
        let mut left = self.additive()?;
        while let Some(t @ (Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge)) = self.peek().cloned() {
            self.pos += 1;
            left = Expr::Compare(t, Box::new(left), Box::new(self.additive()?));
        }
        Ok(left)
    }

    fn additive(&mut self) -> Parse<Expr> {
        let mut left = self.multiplicative()?;
        while let Some(t @ (Tok::Plus | Tok::Minus)) = self.peek().cloned() {
            self.pos += 1;
            left = Expr::Arith(t, Box::new(left), Box::new(self.multiplicative()?));
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Parse<Expr> {
        let mut left = self.unary()?;
        while let Some(t @ (Tok::Star | Tok::Div | Tok::Mod)) = self.peek().cloned() {
            self.pos += 1;
            left = Expr::Arith(t, Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Parse<Expr> {
        if self.eat(&Tok::Minus) {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        let mut left = self.path_expr()?;
        while self.eat(&Tok::Pipe) {
            left = Expr::Union(Box::new(left), Box::new(self.path_expr()?));
        }
        Ok(left)
    }

    fn is_function_call(&self) -> bool {
        match (self.peek(), self.peek_at(1)) {
            (Some(Tok::Name(n)), Some(Tok::LParen)) => !matches!(
                n.as_str(),
                "node" | "text" | "comment" | "processing-instruction"
            ),
            _ => false,
        }
    }

    fn path_expr(&mut self) -> Parse<Expr> {
        let primary = match self.peek() {
            Some(Tok::Literal(_)) | Some(Tok::Number(_)) | Some(Tok::LParen) => true,
            _ => self.is_function_call(),
        };
        if !primary {
            return self.location_path(None);
        }
        let mut e = match self.peek().cloned() {
            Some(Tok::Literal(s)) => {
                self.pos += 1;
                Expr::Literal(s)
            }
            Some(Tok::Number(n)) => {
                self.pos += 1;
                Expr::Number(n)
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                e
            }
            Some(Tok::Name(name)) => {
                self.pos += 2;
                let mut args = Vec::new();
                if !self.eat(&Tok::RParen) {
                    loop {
                        args.push(self.expr()?);
                        if self.eat(&Tok::RParen) {
                            break;
                        }
                        self.expect(&Tok::Comma)?;
                    }
                }
                Expr::Call(name, args)
            }
            _ => return Err(()),
        };
        let predicates = self.predicates()?;
        if !predicates.is_empty() {
            e = Expr::Filter(Box::new(e), predicates);
        }
        if matches!(self.peek(), Some(Tok::Slash) | Some(Tok::DoubleSlash)) {
            return self.location_path(Some(e));
        }
        Ok(e)
    }

    fn predicates(&mut self) -> Parse<Vec<Expr>> {
        let mut out = Vec::new();
        while self.eat(&Tok::LBracket) {
            out.push(self.expr()?);
            self.expect(&Tok::RBracket)?;
        }
        Ok(out)
    }

    fn location_path(&mut self, start: Option<Expr>) -> Parse<Expr> {
        let mut steps = Vec::new();
        let mut absolute = false;
        let descendant = Step {
            axis: Axis::DescendantOrSelf,
            test: NodeTest::Node,
            predicates: Vec::new(),
        };
        let mut need_step = true;
        if start.is_none() {
            if self.eat(&Tok::Slash) {
                absolute = true;
                // `/` alone selects the root.
                need_step = self.starts_step();
            } else if self.eat(&Tok::DoubleSlash) {
                absolute = true;
                steps.push(descendant.clone());
            }
        } else if self.eat(&Tok::DoubleSlash) {
            steps.push(descendant.clone());
        } else {
            self.expect(&Tok::Slash)?;
        }
        if need_step {
            steps.push(self.step()?);
            loop {
                if self.eat(&Tok::Slash) {
                    steps.push(self.step()?);
                } else if self.eat(&Tok::DoubleSlash) {
                    steps.push(descendant.clone());
                    steps.push(self.step()?);
                } else {
                    break;
                }
            }
        }
        Ok(Expr::Path {
            start: start.map(Box::new),
            absolute,
            steps,
        })
    }

    fn starts_step(&self) -> bool {
        matches!(
            self.peek(),
            Some(Tok::Name(_)) | Some(Tok::At) | Some(Tok::Dot) | Some(Tok::DotDot)
        )
    }

    fn step(&mut self) -> Parse<Step> {
        if self.eat(&Tok::Dot) {
            return Ok(Step {
                axis: Axis::SelfAxis,
                test: NodeTest::Node,
                predicates: Vec::new(),
            });
        }
        if self.eat(&Tok::DotDot) {
            return Ok(Step {
                axis: Axis::Parent,
                test: NodeTest::Node,
                predicates: Vec::new(),
            });
        }
        let mut axis = Axis::Child;
        if self.eat(&Tok::At) {
            axis = Axis::Attribute;
        } else if let (Some(Tok::Name(name)), Some(Tok::DoubleColon)) =
            (self.peek().cloned(), self.peek_at(1))
        {
            axis = match name.as_str() {
                "child" => Axis::Child,
                "descendant" => Axis::Descendant,
                "descendant-or-self" => Axis::DescendantOrSelf,
                "self" => Axis::SelfAxis,
                "parent" => Axis::Parent,
                "ancestor" => Axis::Ancestor,
                "ancestor-or-self" => Axis::AncestorOrSelf,
                "following-sibling" => Axis::FollowingSibling,
                "preceding-sibling" => Axis::PrecedingSibling,
                "following" => Axis::Following,
                "preceding" => Axis::Preceding,
                "attribute" => Axis::Attribute,
                _ => return Err(()),
            };
            self.pos += 2;
        }
        let test = match self.peek().cloned() {
            Some(Tok::Name(name)) => {
                self.pos += 1;
                if self.eat(&Tok::LParen) {
                    self.expect(&Tok::RParen)?;
                    match name.as_str() {
                        "node" => NodeTest::Node,
                        "text" => NodeTest::Text,
                        "comment" => NodeTest::Comment,
                        _ => return Err(()),
                    }
                } else if name == "*" {
                    NodeTest::Name(None)
                } else if name.contains(':') {
                    // A prefix Excel can't declare.
                    return Err(());
                } else {
                    NodeTest::Name(Some(name))
                }
            }
            _ => return Err(()),
        };
        Ok(Step {
            axis,
            test,
            predicates: self.predicates()?,
        })
    }
}

// ── Evaluation ───────────────────────────────────────────────────────────

/// A node of the document, or an attribute (its element and index).
#[derive(Clone, Copy, Debug)]
enum Item<'a, 'i> {
    Node(XmlNode<'a, 'i>),
    Attr(XmlNode<'a, 'i>, usize),
}

impl<'a, 'i> Item<'a, 'i> {
    /// Document order: an element, its attributes, then its content.
    fn key(&self) -> (usize, usize) {
        match self {
            Item::Node(n) => (n.id().get_usize(), 0),
            Item::Attr(n, i) => (n.id().get_usize(), i + 1),
        }
    }

    fn string_value(&self) -> String {
        match self {
            Item::Attr(n, i) => n
                .attributes()
                .nth(*i)
                .map_or(String::new(), |a| a.value().to_string()),
            Item::Node(n) => match n.node_type() {
                NodeType::Text | NodeType::Comment => n.text().unwrap_or("").to_string(),
                NodeType::PI => n.pi().and_then(|p| p.value).unwrap_or("").to_string(),
                _ => n
                    .descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect(),
            },
        }
    }

    fn name(&self) -> String {
        match self {
            Item::Attr(n, i) => n
                .attributes()
                .nth(*i)
                .map_or(String::new(), |a| a.name().to_string()),
            Item::Node(n) if n.is_element() => n.tag_name().name().to_string(),
            _ => String::new(),
        }
    }
}

#[derive(Clone, Debug)]
enum Value<'a, 'i> {
    Nodes(Vec<Item<'a, 'i>>),
    Num(f64),
    Str(String),
    Bool(bool),
}

fn number_of(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() || t.contains(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')) {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
    } else if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

impl<'a, 'i> Value<'a, 'i> {
    fn string(&self) -> String {
        match self {
            Value::Nodes(v) => v.first().map_or(String::new(), Item::string_value),
            Value::Num(n) => format_number(*n),
            Value::Str(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
        }
    }

    fn number(&self) -> f64 {
        match self {
            Value::Num(n) => *n,
            Value::Bool(b) => f64::from(u8::from(*b)),
            _ => number_of(&self.string()),
        }
    }

    fn boolean(&self) -> bool {
        match self {
            Value::Nodes(v) => !v.is_empty(),
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::Bool(b) => *b,
        }
    }
}

struct Ctx<'a, 'i> {
    item: Item<'a, 'i>,
    position: usize,
    size: usize,
}

fn sort_dedup<'a, 'i>(items: &mut Vec<Item<'a, 'i>>) {
    items.sort_by_key(Item::key);
    items.dedup_by_key(|i| i.key());
}

fn test_node(test: &NodeTest, axis: Axis, item: &Item) -> bool {
    match item {
        Item::Attr(_, _) => match test {
            NodeTest::Name(None) | NodeTest::Node => true,
            NodeTest::Name(Some(name)) => item.name() == *name,
            _ => false,
        },
        Item::Node(n) => match test {
            NodeTest::Node => true,
            NodeTest::Text => n.is_text(),
            NodeTest::Comment => n.is_comment(),
            NodeTest::Name(name) => {
                // The principal node type of every axis here is element.
                n.is_element()
                    && axis != Axis::Attribute
                    && match name {
                        None => true,
                        Some(name) => {
                            n.tag_name().namespace().is_none() && n.tag_name().name() == name
                        }
                    }
            }
        },
    }
}

fn axis_items<'a, 'i>(axis: Axis, item: Item<'a, 'i>) -> Vec<Item<'a, 'i>> {
    let node = match item {
        Item::Node(n) => n,
        Item::Attr(owner, _) => {
            return match axis {
                Axis::SelfAxis => vec![item],
                Axis::Parent => vec![Item::Node(owner)],
                Axis::Ancestor | Axis::AncestorOrSelf => {
                    let mut v: Vec<Item> = if axis == Axis::AncestorOrSelf {
                        vec![item]
                    } else {
                        Vec::new()
                    };
                    v.extend(owner.ancestors().map(Item::Node));
                    v
                }
                _ => Vec::new(),
            };
        }
    };
    let nodes =
        |it: &mut dyn Iterator<Item = XmlNode<'a, 'i>>| it.map(Item::Node).collect::<Vec<_>>();
    match axis {
        Axis::Child => nodes(&mut node.children()),
        Axis::Descendant => nodes(&mut node.descendants().skip(1)),
        Axis::DescendantOrSelf => nodes(&mut node.descendants()),
        Axis::SelfAxis => vec![item],
        Axis::Parent => nodes(&mut node.parent().into_iter()),
        Axis::Ancestor => nodes(&mut node.ancestors().skip(1)),
        Axis::AncestorOrSelf => nodes(&mut node.ancestors()),
        Axis::FollowingSibling => nodes(&mut node.next_siblings().skip(1)),
        Axis::PrecedingSibling => nodes(&mut node.prev_siblings().skip(1)),
        Axis::Following => {
            let mut out = Vec::new();
            let mut cur = Some(node);
            while let Some(n) = cur {
                for s in n.next_siblings().skip(1) {
                    out.extend(s.descendants().map(Item::Node));
                }
                cur = n.parent();
            }
            out
        }
        Axis::Preceding => {
            let ancestors: Vec<_> = node.ancestors().map(|a| a.id()).collect();
            let root = node.document().root();
            root.descendants()
                .take_while(|d| d.id() != node.id())
                .filter(|d| !ancestors.contains(&d.id()))
                .map(Item::Node)
                .collect()
        }
        Axis::Attribute => {
            if node.is_element() {
                (0..node.attributes().len())
                    .map(|i| Item::Attr(node, i))
                    .collect()
            } else {
                Vec::new()
            }
        }
    }
}

/// Reverse axes count positions from the context node backwards.
fn is_reverse(axis: Axis) -> bool {
    matches!(
        axis,
        Axis::Ancestor | Axis::AncestorOrSelf | Axis::PrecedingSibling | Axis::Preceding
    )
}

struct Eval<'a, 'i> {
    doc: &'a Document<'i>,
}

impl<'a, 'i> Eval<'a, 'i> {
    fn filter(&self, items: Vec<Item<'a, 'i>>, predicates: &[Expr]) -> Parse<Vec<Item<'a, 'i>>> {
        let mut items = items;
        for p in predicates {
            let size = items.len();
            let mut kept = Vec::new();
            for (i, item) in items.iter().enumerate() {
                let ctx = Ctx {
                    item: *item,
                    position: i + 1,
                    size,
                };
                let keep = match self.eval(p, &ctx)? {
                    Value::Num(n) => n == (i + 1) as f64,
                    v => v.boolean(),
                };
                if keep {
                    kept.push(*item);
                }
            }
            items = kept;
        }
        Ok(items)
    }

    fn step(&self, from: &[Item<'a, 'i>], step: &Step) -> Parse<Vec<Item<'a, 'i>>> {
        let mut out = Vec::new();
        for item in from {
            let mut candidates: Vec<Item> = axis_items(step.axis, *item)
                .into_iter()
                .filter(|c| test_node(&step.test, step.axis, c))
                .collect();
            if is_reverse(step.axis) {
                candidates.sort_by_key(Item::key);
                candidates.reverse();
            }
            out.extend(self.filter(candidates, &step.predicates)?);
        }
        sort_dedup(&mut out);
        Ok(out)
    }

    fn nodes(&self, e: &Expr, ctx: &Ctx<'a, 'i>) -> Parse<Vec<Item<'a, 'i>>> {
        match self.eval(e, ctx)? {
            Value::Nodes(v) => Ok(v),
            _ => Err(()),
        }
    }

    fn eval(&self, e: &Expr, ctx: &Ctx<'a, 'i>) -> Parse<Value<'a, 'i>> {
        Ok(match e {
            Expr::Literal(s) => Value::Str(s.clone()),
            Expr::Number(n) => Value::Num(*n),
            Expr::Or(a, b) => {
                Value::Bool(self.eval(a, ctx)?.boolean() || self.eval(b, ctx)?.boolean())
            }
            Expr::And(a, b) => {
                Value::Bool(self.eval(a, ctx)?.boolean() && self.eval(b, ctx)?.boolean())
            }
            Expr::Neg(a) => Value::Num(-self.eval(a, ctx)?.number()),
            Expr::Arith(op, a, b) => {
                let (x, y) = (self.eval(a, ctx)?.number(), self.eval(b, ctx)?.number());
                Value::Num(match op {
                    Tok::Plus => x + y,
                    Tok::Minus => x - y,
                    Tok::Star => x * y,
                    Tok::Div => x / y,
                    _ => x % y,
                })
            }
            Expr::Compare(op, a, b) => {
                Value::Bool(compare(op, &self.eval(a, ctx)?, &self.eval(b, ctx)?))
            }
            Expr::Union(a, b) => {
                let mut v = self.nodes(a, ctx)?;
                v.extend(self.nodes(b, ctx)?);
                sort_dedup(&mut v);
                Value::Nodes(v)
            }
            Expr::Filter(base, predicates) => {
                let mut v = self.nodes(base, ctx)?;
                sort_dedup(&mut v);
                Value::Nodes(self.filter(v, predicates)?)
            }
            Expr::Path {
                start,
                absolute,
                steps,
            } => {
                let mut current = match start {
                    Some(s) => self.nodes(s, ctx)?,
                    None if *absolute => vec![Item::Node(self.doc.root())],
                    None => vec![ctx.item],
                };
                for step in steps {
                    current = self.step(&current, step)?;
                }
                Value::Nodes(current)
            }
            Expr::Call(name, args) => self.call(name, args, ctx)?,
        })
    }

    fn call(&self, name: &str, args: &[Expr], ctx: &Ctx<'a, 'i>) -> Parse<Value<'a, 'i>> {
        let arg = |i: usize| -> Parse<Value<'a, 'i>> {
            match args.get(i) {
                Some(e) => self.eval(e, ctx),
                None => Ok(Value::Nodes(vec![ctx.item])),
            }
        };
        let s = |i: usize| -> Parse<String> { Ok(arg(i)?.string()) };
        let n = |i: usize| -> Parse<f64> { Ok(arg(i)?.number()) };
        let count = |min: usize, max: usize| {
            if args.len() < min || args.len() > max {
                Err(())
            } else {
                Ok(())
            }
        };
        Ok(match name {
            "last" => {
                count(0, 0)?;
                Value::Num(ctx.size as f64)
            }
            "position" => {
                count(0, 0)?;
                Value::Num(ctx.position as f64)
            }
            "count" => {
                count(1, 1)?;
                Value::Num(self.nodes(&args[0], ctx)?.len() as f64)
            }
            "name" | "local-name" => {
                count(0, 1)?;
                let v = match arg(0)? {
                    Value::Nodes(v) => v,
                    _ => return Err(()),
                };
                Value::Str(v.first().map_or(String::new(), Item::name))
            }
            "string" => {
                count(0, 1)?;
                Value::Str(s(0)?)
            }
            "concat" => {
                if args.len() < 2 {
                    return Err(());
                }
                let mut out = String::new();
                for e in args {
                    out.push_str(&self.eval(e, ctx)?.string());
                }
                Value::Str(out)
            }
            "starts-with" => {
                count(2, 2)?;
                Value::Bool(s(0)?.starts_with(&s(1)?))
            }
            "contains" => {
                count(2, 2)?;
                Value::Bool(s(0)?.contains(&s(1)?))
            }
            "substring-before" => {
                count(2, 2)?;
                let (a, b) = (s(0)?, s(1)?);
                Value::Str(a.find(&b).map_or(String::new(), |i| a[..i].to_string()))
            }
            "substring-after" => {
                count(2, 2)?;
                let (a, b) = (s(0)?, s(1)?);
                Value::Str(
                    a.find(&b)
                        .map_or(String::new(), |i| a[i + b.len()..].to_string()),
                )
            }
            "substring" => {
                count(2, 3)?;
                let text: Vec<char> = s(0)?.chars().collect();
                let start = round_half_up(n(1)?);
                let end = if args.len() == 3 {
                    start + round_half_up(n(2)?)
                } else {
                    f64::INFINITY
                };
                Value::Str(
                    text.iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            let p = (*i + 1) as f64;
                            p >= start && p < end
                        })
                        .map(|(_, c)| *c)
                        .collect(),
                )
            }
            "string-length" => {
                count(0, 1)?;
                Value::Num(s(0)?.chars().count() as f64)
            }
            "normalize-space" => {
                count(0, 1)?;
                Value::Str(s(0)?.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            "translate" => {
                count(3, 3)?;
                let (text, from, to) = (s(0)?, s(1)?, s(2)?);
                let from: Vec<char> = from.chars().collect();
                let to: Vec<char> = to.chars().collect();
                Value::Str(
                    text.chars()
                        .filter_map(|c| match from.iter().position(|&f| f == c) {
                            Some(i) => to.get(i).copied(),
                            None => Some(c),
                        })
                        .collect(),
                )
            }
            "not" => {
                count(1, 1)?;
                Value::Bool(!arg(0)?.boolean())
            }
            "true" => {
                count(0, 0)?;
                Value::Bool(true)
            }
            "false" => {
                count(0, 0)?;
                Value::Bool(false)
            }
            "boolean" => {
                count(1, 1)?;
                Value::Bool(arg(0)?.boolean())
            }
            "number" => {
                count(0, 1)?;
                Value::Num(n(0)?)
            }
            "sum" => {
                count(1, 1)?;
                Value::Num(
                    self.nodes(&args[0], ctx)?
                        .iter()
                        .map(|i| number_of(&i.string_value()))
                        .sum(),
                )
            }
            "floor" => {
                count(1, 1)?;
                Value::Num(n(0)?.floor())
            }
            "ceiling" => {
                count(1, 1)?;
                Value::Num(n(0)?.ceil())
            }
            "round" => {
                count(1, 1)?;
                Value::Num(round_half_up(n(0)?))
            }
            _ => return Err(()),
        })
    }
}

fn round_half_up(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        x
    } else {
        (x + 0.5).floor()
    }
}

/// XPath 1.0 comparison, node-sets compared member by member.
fn compare(op: &Tok, a: &Value, b: &Value) -> bool {
    let scalar = |x: &Value, y: &Value| -> bool {
        match op {
            Tok::Eq | Tok::Ne => {
                let equal = match (x, y) {
                    (Value::Bool(_), _) | (_, Value::Bool(_)) => x.boolean() == y.boolean(),
                    (Value::Num(_), _) | (_, Value::Num(_)) => x.number() == y.number(),
                    _ => x.string() == y.string(),
                };
                if *op == Tok::Eq {
                    equal
                } else {
                    !equal
                }
            }
            _ => {
                let (p, q) = (x.number(), y.number());
                match op {
                    Tok::Lt => p < q,
                    Tok::Le => p <= q,
                    Tok::Gt => p > q,
                    _ => p >= q,
                }
            }
        }
    };
    let members = |v: &Value| -> Option<Vec<Value<'static, 'static>>> {
        match v {
            Value::Nodes(items) => {
                Some(items.iter().map(|i| Value::Str(i.string_value())).collect())
            }
            _ => None,
        }
    };
    match (members(a), members(b)) {
        (Some(xs), Some(ys)) => xs.iter().any(|x| ys.iter().any(|y| scalar(x, y))),
        (Some(xs), None) => {
            if let Value::Bool(_) = b {
                return scalar(&Value::Bool(!xs.is_empty()), b);
            }
            xs.iter().any(|x| scalar(x, b))
        }
        (None, Some(ys)) => {
            if let Value::Bool(_) = a {
                return scalar(a, &Value::Bool(!ys.is_empty()));
            }
            ys.iter().any(|y| scalar(a, y))
        }
        (None, None) => scalar(a, b),
    }
}

/// The values FILTERXML returns for `xml` and `xpath`, or `None` (#VALUE!).
pub(crate) fn filter_xml(xml: &str, xpath: &str) -> Option<Vec<String>> {
    let doc = Document::parse(xml).ok()?;
    let toks = tokenize(xpath)?;
    let mut parser = Parser { toks, pos: 0 };
    let expr = parser.expr().ok()?;
    if parser.pos != parser.toks.len() {
        return None;
    }
    let eval = Eval { doc: &doc };
    let ctx = Ctx {
        item: Item::Node(doc.root()),
        position: 1,
        size: 1,
    };
    match eval.eval(&expr, &ctx).ok()? {
        Value::Nodes(items) if !items.is_empty() => {
            Some(items.iter().map(Item::string_value).collect())
        }
        _ => None,
    }
}

/// A value of FILTERXML: a number when the text reads as one.
fn cell_value(text: String) -> ArrayNode {
    let t = text.trim();
    let numeric = !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
        && t.chars().any(|c| c.is_ascii_digit());
    match t.parse::<f64>() {
        Ok(n) if numeric && n.is_finite() => ArrayNode::Number(n),
        _ => ArrayNode::String(text),
    }
}

impl Model<'_> {
    /// `FILTERXML(xml, xpath)`.
    pub(crate) fn fn_filterxml(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() != 2 {
            return CalcResult::new_args_number_error(cell);
        }
        let xml = match self.get_string(&args[0], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let xpath = match self.get_string(&args[1], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let Some(values) = filter_xml(&xml, &xpath) else {
            return CalcResult::new_error(
                Error::VALUE,
                cell,
                "Invalid XML or XPath, or nothing matched".to_string(),
            );
        };
        let mut rows: Vec<Vec<ArrayNode>> =
            values.into_iter().map(|v| vec![cell_value(v)]).collect();
        if rows.len() == 1 {
            return match rows.remove(0).remove(0) {
                ArrayNode::Number(n) => CalcResult::Number(n),
                ArrayNode::String(s) => CalcResult::String(s),
                _ => CalcResult::new_error(Error::VALUE, cell, String::new()),
            };
        }
        CalcResult::Array(rows)
    }
}
