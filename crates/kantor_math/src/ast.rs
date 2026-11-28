//! The equation model shared by every format: close to Office Math
//! (OMML), which is the richest of them, so a Word equation survives the
//! trip through it.

/// A sequence of math items (OMML's `m:e`, `m:num`… arguments).
pub type Row = Vec<Node>;

/// An equation: `m:oMath` (inline) or `m:oMathPara` (display).
#[derive(Clone, Debug, PartialEq)]
pub struct Equation {
    /// Display math (`m:oMathPara`, LaTeX `$$…$$`): on a line of its own,
    /// big operators with limits above and below.
    pub display: bool,
    /// Where display lines sit on their line (`m:oMathParaPr/m:jc`).
    pub justify: Justify,
    /// One row per line (`m:oMath` inside `m:oMathPara`); inline
    /// equations have one.
    pub lines: Vec<Row>,
}

impl Equation {
    pub fn inline(row: Row) -> Self {
        Self {
            display: false,
            justify: Justify::CenterGroup,
            lines: vec![row],
        }
    }

    pub fn display(row: Row) -> Self {
        Self {
            display: true,
            justify: Justify::CenterGroup,
            lines: vec![row],
        }
    }

    /// No content at all.
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }
}

/// `m:jc` of a math paragraph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Justify {
    Left,
    Right,
    Center,
    /// Word's default: the lines centred as a group, left-aligned among
    /// themselves.
    #[default]
    CenterGroup,
}

impl Justify {
    pub fn omml(self) -> &'static str {
        match self {
            Justify::Left => "left",
            Justify::Right => "right",
            Justify::Center => "center",
            Justify::CenterGroup => "centerGroup",
        }
    }

    pub fn from_omml(v: &str) -> Self {
        match v {
            "left" => Justify::Left,
            "right" => Justify::Right,
            "center" => Justify::Center,
            _ => Justify::CenterGroup,
        }
    }
}

/// One math item.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// Text (`m:r`): letters, digits, operators.
    Text(Run),
    /// `m:f`.
    Frac { kind: FracKind, num: Row, den: Row },
    /// `m:sSub`, `m:sSup`, `m:sSubSup`.
    Scripts {
        base: Row,
        sub: Option<Row>,
        sup: Option<Row>,
    },
    /// `m:sPre`: scripts before the base.
    PreScripts { sub: Row, sup: Row, base: Row },
    /// `m:rad`; `degree` is `None` for a square root (`m:degHide`).
    Radical { degree: Option<Row>, body: Row },
    /// `m:nary`: ∑, ∫, ∏… with optional limits and the operand.
    Nary {
        op: char,
        /// Limits above and below (`undOvr`) rather than as scripts.
        under_over: bool,
        sub: Option<Row>,
        sup: Option<Row>,
        body: Row,
    },
    /// `m:d`: brackets around one or more items. An empty string is no
    /// bracket on that side.
    Delim {
        open: String,
        close: String,
        sep: String,
        items: Vec<Row>,
        /// The brackets grow with what they hold (`m:grow`, on unless off).
        grow: bool,
    },
    /// `m:m`: rows of cells.
    Matrix { rows: Vec<Vec<Row>> },
    /// `m:eqArr`: rows aligned at `&` (alignment points as `&` text, as
    /// Word stores them), each row may end in `#(number)`.
    EqArray { rows: Vec<Row> },
    /// `m:acc`: an accent over the base (a combining character).
    Accent { chr: char, body: Row },
    /// `m:bar`: a line over (`top`) or under the base.
    Bar { top: bool, body: Row },
    /// `m:borderBox`.
    Boxed { body: Row },
    /// `m:box`: a group with no look of its own.
    Group { body: Row },
    /// `m:groupChr`: a stretched character (brace, arrow) under or over.
    GroupChr { chr: char, top: bool, body: Row },
    /// `m:limLow` / `m:limUpp`.
    Limit { upper: bool, base: Row, limit: Row },
    /// `m:func`: a function name and its argument.
    Func { name: Row, body: Row },
    /// `m:phant`.
    Phantom {
        body: Row,
        show: bool,
        zero_width: bool,
        zero_asc: bool,
        zero_desc: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FracKind {
    #[default]
    Bar,
    /// Slanted, `a⁄b`.
    Skewed,
    /// On one line, `a/b`.
    Linear,
    /// Stacked with no line (binomials).
    NoBar,
}

impl FracKind {
    pub fn omml(self) -> &'static str {
        match self {
            FracKind::Bar => "bar",
            FracKind::Skewed => "skw",
            FracKind::Linear => "lin",
            FracKind::NoBar => "noBar",
        }
    }

    pub fn from_omml(v: &str) -> Self {
        match v {
            "skw" => FracKind::Skewed,
            "lin" => FracKind::Linear,
            "noBar" => FracKind::NoBar,
            _ => FracKind::Bar,
        }
    }
}

/// A text run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

impl Run {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

/// How a run's letters look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    /// `m:scr`: the alphabet.
    pub variant: Variant,
    /// `m:sty`: `None` is math's default (italic letters, upright digits
    /// and operators); `Some` is set explicitly.
    pub weight: Option<Weight>,
    /// `m:nor`: ordinary text (LaTeX `\text{}`), spaces kept, no math
    /// italic.
    pub normal: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        variant: Variant::Roman,
        weight: Some(Weight::Plain),
        normal: false,
    };
    pub const TEXT: Style = Style {
        variant: Variant::Roman,
        weight: Some(Weight::Plain),
        normal: true,
    };

    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }
}

/// `m:sty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Plain,
    Bold,
    Italic,
    BoldItalic,
}

impl Weight {
    pub fn omml(self) -> &'static str {
        match self {
            Weight::Plain => "p",
            Weight::Bold => "b",
            Weight::Italic => "i",
            Weight::BoldItalic => "bi",
        }
    }

    pub fn from_omml(v: &str) -> Option<Self> {
        Some(match v {
            "p" => Weight::Plain,
            "b" => Weight::Bold,
            "i" => Weight::Italic,
            "bi" => Weight::BoldItalic,
            _ => return None,
        })
    }

    pub fn bold(self) -> bool {
        matches!(self, Weight::Bold | Weight::BoldItalic)
    }

    pub fn italic(self) -> bool {
        matches!(self, Weight::Italic | Weight::BoldItalic)
    }
}

/// `m:scr`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Variant {
    #[default]
    Roman,
    Script,
    Fraktur,
    DoubleStruck,
    SansSerif,
    Monospace,
}

impl Variant {
    pub fn omml(self) -> &'static str {
        match self {
            Variant::Roman => "roman",
            Variant::Script => "script",
            Variant::Fraktur => "fraktur",
            Variant::DoubleStruck => "double-struck",
            Variant::SansSerif => "sans-serif",
            Variant::Monospace => "monospace",
        }
    }

    pub fn from_omml(v: &str) -> Self {
        match v {
            "script" => Variant::Script,
            "fraktur" => Variant::Fraktur,
            "double-struck" => Variant::DoubleStruck,
            "sans-serif" => Variant::SansSerif,
            "monospace" => Variant::Monospace,
            _ => Variant::Roman,
        }
    }
}

/// Appends a node to a row, merging text with the run before it when
/// both look the same.
pub fn push(row: &mut Row, node: Node) {
    if let Node::Text(r) = &node {
        if r.text.is_empty() {
            return;
        }
        if let Some(Node::Text(last)) = row.last_mut()
            && last.style == r.style
        {
            last.text.push_str(&r.text);
            return;
        }
    }
    row.push(node);
}

/// A row's text runs merged where they look the same.
pub fn normalize(row: Row) -> Row {
    let mut out = Row::new();
    for n in row {
        push(&mut out, n);
    }
    out
}

/// The plain text of a row, structure dropped (for search and labels).
pub fn plain_text(row: &[Node]) -> String {
    let mut s = String::new();
    for n in row {
        plain_into(n, &mut s);
    }
    s
}

fn plain_rows(rs: &[&Row], s: &mut String) {
    for r in rs {
        for n in r.iter() {
            plain_into(n, s);
        }
    }
}

fn plain_into(n: &Node, s: &mut String) {
    let rows = |rs: &[&Row], s: &mut String| plain_rows(rs, s);
    match n {
        Node::Text(r) => s.push_str(&r.text),
        Node::Frac { num, den, .. } => rows(&[num, den], s),
        Node::Scripts { base, sub, sup } => {
            rows(&[base], s);
            if let Some(r) = sub {
                rows(&[r], s);
            }
            if let Some(r) = sup {
                rows(&[r], s);
            }
        }
        Node::PreScripts { sub, sup, base } => rows(&[sub, sup, base], s),
        Node::Radical { degree, body } => {
            if let Some(d) = degree {
                rows(&[d], s);
            }
            rows(&[body], s);
        }
        Node::Nary {
            op, sub, sup, body, ..
        } => {
            s.push(*op);
            if let Some(r) = sub {
                rows(&[r], s);
            }
            if let Some(r) = sup {
                rows(&[r], s);
            }
            rows(&[body], s);
        }
        Node::Delim {
            open, close, items, ..
        } => {
            s.push_str(open);
            for i in items {
                rows(&[i], s);
            }
            s.push_str(close);
        }
        Node::Matrix { rows: m } => {
            for r in m {
                for c in r {
                    rows(&[c], s);
                }
            }
        }
        Node::EqArray { rows: rs } => {
            for r in rs {
                rows(&[r], s);
            }
        }
        Node::Accent { body, .. }
        | Node::Bar { body, .. }
        | Node::Boxed { body }
        | Node::Group { body }
        | Node::GroupChr { body, .. }
        | Node::Phantom { body, .. } => rows(&[body], s),
        Node::Limit { base, limit, .. } => rows(&[base, limit], s),
        Node::Func { name, body } => rows(&[name, body], s),
    }
}
