//! The VBA syntax tree the parser builds.

use std::rc::Rc;

use crate::object::Value;

/// A name as written, plus its lower-case key (VBA names are case-insensitive).
#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub key: Rc<str>,
    pub text: Rc<str>,
}

impl Ident {
    pub fn new(text: &str) -> Ident {
        Ident {
            key: Rc::from(text.to_lowercase()),
            text: Rc::from(text),
        }
    }

    pub fn is(&self, key: &str) -> bool {
        &*self.key == key
    }
}

/// A declared type. `Named` covers classes, user-defined types and host object types
/// (`Range`, `Scripting.Dictionary`); it's resolved when used. The name is lower case.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Variant,
    Boolean,
    Byte,
    Integer,
    Long,
    LongLong,
    Single,
    Double,
    Currency,
    Date,
    String,
    /// `String * n`.
    FixedString(usize),
    Object,
    Named(Rc<str>),
    /// An array of the element type (`Dim a() As Long`, a `Long()` parameter).
    Array(Box<Ty>),
}

impl Ty {
    pub fn is_object(&self) -> bool {
        matches!(self, Ty::Object | Ty::Named(_))
    }

    /// The type a type-declaration character stands for (`s$`, `n%`, `x#`).
    pub fn from_suffix(c: char) -> Option<Ty> {
        Some(match c {
            '%' => Ty::Integer,
            '&' => Ty::Long,
            '^' => Ty::LongLong,
            '!' => Ty::Single,
            '#' => Ty::Double,
            '@' => Ty::Currency,
            '$' => Ty::String,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Pow,
    Mul,
    Div,
    IntDiv,
    Mod,
    Add,
    Sub,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Like,
    Is,
    And,
    Or,
    Xor,
    Eqv,
    Imp,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Lit(Value),
    Name(Ident),
    /// `base.member`.
    Member(Box<Expr>, Ident),
    /// `base!key`: the default member called with the string `"key"`.
    Bang(Box<Expr>, Ident),
    /// `target(args)`: a call, an array index or a default-member access, decided at run time.
    Call(Box<Expr>, Vec<ArgExpr>),
    /// A parenthesised expression; as an argument it's passed by value.
    Paren(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `New Class`; the name is lower case and may be dotted.
    New(Rc<str>),
    /// `TypeOf x Is Class`.
    TypeOf(Box<Expr>, Rc<str>),
    /// The object of the enclosing `With` block at this nesting depth (`.Value`).
    With(usize),
    Me,
    /// `[A1]`: the host's `Evaluate`.
    Evaluate(String),
}

impl Expr {
    pub fn name(text: &str) -> Expr {
        Expr::Name(Ident::new(text))
    }
}

/// An argument expression. `value` is `None` for an omitted argument (`Foo 1, , 3`).
#[derive(Clone, Debug, PartialEq)]
pub struct ArgExpr {
    pub name: Option<Ident>,
    pub value: Option<Expr>,
}

/// One array dimension: `[lower To] upper`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bounds {
    pub lower: Option<Expr>,
    pub upper: Expr,
}

/// A variable declaration (`Dim`, `Private`, `Static`, a `Type` field).
#[derive(Clone, Debug, PartialEq)]
pub struct VarDecl {
    pub name: Ident,
    /// The element type for arrays.
    pub ty: Ty,
    /// `None`: a scalar; `Some(empty)`: a dynamic array; otherwise a fixed-size array.
    pub array: Option<Vec<Bounds>>,
    /// `As New Class`: created on first use.
    pub auto_new: bool,
    /// `WithEvents`: the object's events call `<name>_<Event>` procedures of the module.
    pub with_events: bool,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: Ty,
    pub value: Expr,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: Ty,
    pub byval: bool,
    pub optional: bool,
    pub default: Option<Expr>,
    pub paramarray: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcKind {
    Sub,
    Function,
    PropertyGet,
    PropertyLet,
    PropertySet,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaseClause {
    Value(Expr),
    Range(Expr, Expr),
    /// `Is <op> expr`.
    Is(BinOp, Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum OnError {
    ResumeNext,
    GoTo(Rc<str>),
    /// `On Error GoTo 0`.
    Disable,
    /// `On Error GoTo -1`.
    Reset,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resume {
    /// `Resume` / `Resume 0`: retry the statement that failed.
    Here,
    Next,
    Label(Rc<str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    For,
    Do,
    Proc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoKind {
    /// `Do ... Loop`.
    Forever,
    PreWhile,
    PreUntil,
    PostWhile,
    PostUntil,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrintItem {
    Expr(Expr),
    /// `;`: no separation.
    Semi,
    /// `,`: next 14-column print zone.
    Comma,
    Spc(Expr),
    Tab(Option<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReDimItem {
    pub target: Expr,
    pub bounds: Vec<Bounds>,
    pub ty: Option<Ty>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfArm {
    pub cond: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaseArm {
    pub clauses: Vec<CaseClause>,
    pub body: Vec<Stmt>,
}

/// A statement with its source line.
#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub line: u32,
    pub kind: StmtKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    Assign {
        set: bool,
        target: Expr,
        value: Expr,
    },
    /// A call statement; `target` has no argument list of its own unless written `Foo(x)`.
    Call {
        target: Expr,
        args: Vec<ArgExpr>,
    },
    Dim {
        decls: Vec<VarDecl>,
        is_static: bool,
    },
    Const(Vec<ConstDecl>),
    ReDim {
        preserve: bool,
        items: Vec<ReDimItem>,
    },
    Erase(Vec<Expr>),
    /// `Mid(target, start[, length]) = value`.
    MidAssign {
        target: Expr,
        start: Expr,
        length: Option<Expr>,
        value: Expr,
    },
    /// `LSet` (`right` false) / `RSet` (`right` true).
    Pad {
        right: bool,
        target: Expr,
        value: Expr,
    },
    If {
        arms: Vec<IfArm>,
        otherwise: Option<Vec<Stmt>>,
    },
    Select {
        test: Expr,
        arms: Vec<CaseArm>,
        otherwise: Option<Vec<Stmt>>,
    },
    For {
        var: Expr,
        from: Expr,
        to: Expr,
        step: Option<Expr>,
        body: Vec<Stmt>,
    },
    ForEach {
        var: Expr,
        collection: Expr,
        body: Vec<Stmt>,
    },
    Do {
        kind: DoKind,
        cond: Option<Expr>,
        body: Vec<Stmt>,
    },
    With {
        object: Expr,
        depth: usize,
        body: Vec<Stmt>,
    },
    Label(Rc<str>),
    GoTo(Rc<str>),
    GoSub(Rc<str>),
    Return,
    /// `On expr GoTo a, b, c` (`gosub`: `On expr GoSub ...`).
    OnJump {
        index: Expr,
        labels: Vec<Rc<str>>,
        gosub: bool,
    },
    /// `RaiseEvent Name(args)`.
    RaiseEvent {
        name: Ident,
        args: Vec<ArgExpr>,
    },
    OnError(OnError),
    Resume(Resume),
    Exit(ExitKind),
    /// `Error n`.
    Error(Expr),
    Print(Vec<PrintItem>),
    Assert(Expr),
    Stop,
    End,
    /// A statement Kantor parses but doesn't run; executing it raises an error naming it.
    Unsupported(String),
    /// A line that didn't parse; executing it raises a syntax error.
    Invalid(String),
    /// Nothing to run (an `Attribute` line inside a procedure).
    Nop,
}

/// `Type Name ... End Type`.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub name: Ident,
    pub fields: Vec<VarDecl>,
}

/// `Enum Name ... End Enum`.
#[derive(Clone, Debug, PartialEq)]
pub struct EnumDef {
    pub name: Ident,
    pub public: bool,
    pub members: Vec<(Ident, Option<Expr>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcDef {
    pub name: Ident,
    pub kind: ProcKind,
    pub public: bool,
    /// `Static Sub`: every local keeps its value between calls.
    pub is_static: bool,
    pub params: Vec<Param>,
    pub ret: Ty,
    pub body: Vec<Stmt>,
    pub line: u32,
    /// `Attribute Name.VB_UserMemId = 0`: the class's default member.
    pub default_member: bool,
    /// `Attribute Name.VB_UserMemId = -4`: the class's enumerator (`For Each`).
    pub enumerator: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModuleItem {
    Var {
        public: bool,
        decl: VarDecl,
    },
    Const {
        public: bool,
        decl: ConstDecl,
    },
    Type(TypeDef),
    Enum(EnumDef),
    /// `Declare Function name Lib "lib"`: parsed so calls fail with a clear message.
    Declare {
        name: Ident,
        lib: String,
    },
    Proc(ProcDef),
    /// `Event Name(params)`: an event the class raises.
    Event(Ident),
    /// `Implements Interface`: the lower-case class name.
    Implements(Rc<str>),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ModuleDef {
    pub option_explicit: bool,
    pub option_base: i32,
    pub option_compare_text: bool,
    pub option_private: bool,
    /// `DefInt A-Z` and friends: the implicit type per initial letter.
    pub deftypes: Vec<(char, char, Ty)>,
    pub items: Vec<ModuleItem>,
}
