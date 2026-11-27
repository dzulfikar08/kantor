//! Turns parsed procedures into a flat list of operations with jumps.
//!
//! Flat code makes VBA's unstructured control flow simple: `GoTo`, `On Error GoTo`, `Resume`,
//! `Resume Next` ("the statement after the one that failed", even into an `If` body, as VBA
//! does) and `GoSub` are jumps to operation indices. Expressions stay trees.
//!
//! `Dim`, `Static` and `Const` inside a procedure are hoisted: VBA allocates every local when
//! the procedure starts, wherever the `Dim` is (MS-VBAL §5.2.3.1, a `Dim` inside a loop doesn't
//! reset the variable on each pass).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::ast::*;
use crate::object::{Location, ScriptError};

/// What a module is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    /// A standard module: public members are global.
    Standard,
    /// A class module: members belong to instances made with `New`.
    Class,
    /// A document module (`ThisWorkbook`, `Sheet1`): procedures are reached by qualification;
    /// `Me` is the host object of the same name.
    Document,
    /// A UserForm's code; forms can't be shown, so running it fails with a clear error.
    Form,
}

#[derive(Debug)]
pub enum ErrMode {
    ResumeNext,
    GoTo(usize),
    Disable,
    Reset,
}

#[derive(Debug)]
pub enum ResumeTo {
    Here,
    Next,
    Label(usize),
}

#[derive(Debug)]
pub struct ForOp {
    pub var: Expr,
    pub from: Expr,
    pub to: Expr,
    pub step: Option<Expr>,
    pub slot: usize,
    pub exit: usize,
}

#[derive(Debug)]
pub struct EachOp {
    pub var: Expr,
    pub collection: Expr,
    pub slot: usize,
    pub exit: usize,
}

#[derive(Debug)]
pub struct NextOp {
    pub var: Expr,
    pub slot: usize,
    pub body: usize,
}

#[derive(Debug)]
pub enum Op {
    /// A simple statement.
    Exec(StmtKind),
    /// Jumps to `target` when the condition's truth equals `when`.
    Branch {
        cond: Expr,
        when: bool,
        target: usize,
    },
    Jump(usize),
    For(Box<ForOp>),
    Next(Box<NextOp>),
    Each(Box<EachOp>),
    EachNext(Box<NextOp>),
    Select {
        test: Expr,
        slot: usize,
    },
    /// Falls through when a clause matches, else jumps to `next`.
    Case {
        slot: usize,
        clauses: Vec<CaseClause>,
        next: usize,
    },
    WithEnter {
        depth: usize,
        object: Expr,
    },
    WithLeave(usize),
    OnError(ErrMode),
    Resume(ResumeTo),
    GoSub(usize),
    /// `On index GoTo/GoSub labels`: jumps to `targets[index - 1]`, or falls through when
    /// `index` is 0 or past the list.
    OnJump {
        index: Expr,
        labels: Vec<Rc<str>>,
        targets: Vec<usize>,
        gosub: bool,
    },
    Return,
    /// Leaves the procedure.
    Exit,
}

/// A compiled procedure.
#[derive(Debug)]
pub struct Proc {
    pub name: Ident,
    pub kind: ProcKind,
    pub public: bool,
    pub is_static: bool,
    pub params: Vec<Param>,
    pub ret: Ty,
    pub ops: Vec<Op>,
    /// Source line of each op.
    pub lines: Vec<u32>,
    /// The last line-number label before each op (`Erl`).
    pub erl: Vec<u32>,
    pub locals: Vec<VarDecl>,
    pub statics: Vec<VarDecl>,
    pub consts: Vec<ConstDecl>,
    /// Loop and `Select Case` state slots.
    pub slots: usize,
    pub line: u32,
}

/// The procedures sharing a name (a `Property Get`/`Let`/`Set` trio, or one Sub/Function).
#[derive(Clone, Debug, Default)]
pub struct ProcGroup {
    pub sub: Option<Rc<Proc>>,
    pub get: Option<Rc<Proc>>,
    pub let_: Option<Rc<Proc>>,
    pub set: Option<Rc<Proc>>,
}

impl ProcGroup {
    pub fn any(&self) -> Option<&Rc<Proc>> {
        self.sub
            .as_ref()
            .or(self.get.as_ref())
            .or(self.let_.as_ref())
            .or(self.set.as_ref())
    }

    pub fn is_public(&self) -> bool {
        [&self.sub, &self.get, &self.let_, &self.set]
            .into_iter()
            .flatten()
            .any(|p| p.public)
    }
}

/// A compiled module.
#[derive(Debug)]
pub struct Module {
    pub idx: usize,
    pub name: String,
    pub key: Rc<str>,
    pub kind: ModuleKind,
    pub explicit: bool,
    pub base: i32,
    pub compare_text: bool,
    pub deftypes: Vec<(char, char, Ty)>,
    pub vars: Vec<VarDecl>,
    pub public_vars: HashSet<Rc<str>>,
    /// Constants and enum members in declaration order, with their visibility.
    pub consts: Vec<(bool, ConstDecl)>,
    pub enums: Vec<EnumDef>,
    pub types: HashMap<Rc<str>, Rc<TypeDef>>,
    pub procs: HashMap<Rc<str>, ProcGroup>,
    /// Procedures in source order.
    pub order: Vec<Rc<Proc>>,
    pub declares: HashMap<Rc<str>, String>,
    pub default_member: Option<Rc<str>>,
    pub enumerator: Option<Rc<str>>,
    /// `Implements` clauses (lower-case class names).
    pub implements: Vec<Rc<str>>,
    /// `Event` declarations (lower-case names).
    pub events: Vec<Rc<str>>,
    /// Module-level variables declared `WithEvents` (lower-case names).
    pub with_events: Vec<Rc<str>>,
    /// An empty procedure: the context for module-level expressions.
    pub init: Rc<Proc>,
}

impl Module {
    /// The implicit type of an undeclared variable (`DefInt A-Z`).
    pub fn deftype(&self, name: &str) -> Ty {
        let Some(c) = name.chars().next().map(|c| c.to_ascii_uppercase()) else {
            return Ty::Variant;
        };
        self.deftypes
            .iter()
            .rev()
            .find(|(a, b, _)| (*a..=*b).contains(&c))
            .map_or(Ty::Variant, |(_, _, t)| t.clone())
    }
}

fn empty_proc() -> Proc {
    Proc {
        name: Ident::new(""),
        kind: ProcKind::Sub,
        public: false,
        is_static: false,
        params: Vec::new(),
        ret: Ty::Variant,
        ops: Vec::new(),
        lines: Vec::new(),
        erl: Vec::new(),
        locals: Vec::new(),
        statics: Vec::new(),
        consts: Vec::new(),
        slots: 0,
        line: 0,
    }
}

pub fn compile_module(
    idx: usize,
    name: &str,
    kind: ModuleKind,
    def: ModuleDef,
) -> Result<Module, ScriptError> {
    let mut m = Module {
        idx,
        name: name.to_string(),
        key: Rc::from(name.to_lowercase()),
        kind,
        explicit: def.option_explicit,
        base: def.option_base,
        compare_text: def.option_compare_text,
        deftypes: def.deftypes,
        vars: Vec::new(),
        public_vars: HashSet::new(),
        consts: Vec::new(),
        enums: Vec::new(),
        types: HashMap::new(),
        procs: HashMap::new(),
        order: Vec::new(),
        declares: HashMap::new(),
        default_member: None,
        enumerator: None,
        implements: Vec::new(),
        events: Vec::new(),
        with_events: Vec::new(),
        init: Rc::new(empty_proc()),
    };
    for item in def.items {
        match item {
            ModuleItem::Var { public, decl } => {
                if public {
                    m.public_vars.insert(decl.name.key.clone());
                }
                if decl.with_events {
                    m.with_events.push(decl.name.key.clone());
                }
                m.vars.push(decl);
            }
            ModuleItem::Const { public, decl } => m.consts.push((public, decl)),
            ModuleItem::Type(t) => {
                m.types.insert(t.name.key.clone(), Rc::new(t));
            }
            ModuleItem::Enum(e) => m.enums.push(e),
            ModuleItem::Declare { name, lib } => {
                m.declares.insert(name.key, lib);
            }
            ModuleItem::Event(name) => m.events.push(name.key),
            ModuleItem::Implements(name) => m.implements.push(Rc::from(name.to_lowercase())),
            ModuleItem::Proc(def) => {
                let key = def.name.key.clone();
                if def.default_member {
                    m.default_member = Some(key.clone());
                }
                if def.enumerator {
                    m.enumerator = Some(key.clone());
                }
                let line = def.line;
                let proc = Rc::new(compile_proc(def));
                let group = m.procs.entry(key).or_default();
                let conflict = match proc.kind {
                    ProcKind::Sub | ProcKind::Function => group.any().is_some(),
                    ProcKind::PropertyGet => group.sub.is_some() || group.get.is_some(),
                    ProcKind::PropertyLet => group.sub.is_some() || group.let_.is_some(),
                    ProcKind::PropertySet => group.sub.is_some() || group.set.is_some(),
                };
                if conflict {
                    let mut e =
                        ScriptError::syntax(format!("Ambiguous name detected: {}", proc.name.text));
                    e.location = Some(Location {
                        module: name.to_string(),
                        procedure: proc.name.text.to_string(),
                        line,
                    });
                    return Err(e);
                }
                let place = match proc.kind {
                    ProcKind::Sub | ProcKind::Function => &mut group.sub,
                    ProcKind::PropertyGet => &mut group.get,
                    ProcKind::PropertyLet => &mut group.let_,
                    ProcKind::PropertySet => &mut group.set,
                };
                *place = Some(proc.clone());
                m.order.push(proc);
            }
        }
    }
    Ok(m)
}

struct Builder {
    ops: Vec<Op>,
    lines: Vec<u32>,
    erl: Vec<u32>,
    erl_now: u32,
    labels: HashMap<Rc<str>, usize>,
    fixups: Vec<(usize, Rc<str>)>,
    loops: Vec<(ExitKind, Vec<usize>)>,
    slots: usize,
    locals: Vec<VarDecl>,
    statics: Vec<VarDecl>,
    consts: Vec<ConstDecl>,
}

const UNSET: usize = usize::MAX;

impl Builder {
    fn emit(&mut self, op: Op, line: u32) -> usize {
        self.ops.push(op);
        self.lines.push(line);
        self.erl.push(self.erl_now);
        self.ops.len() - 1
    }

    fn here(&self) -> usize {
        self.ops.len()
    }

    fn slot(&mut self) -> usize {
        self.slots += 1;
        self.slots - 1
    }

    fn patch(&mut self, at: usize, target: usize) {
        match &mut self.ops[at] {
            Op::Jump(t) | Op::GoSub(t) => *t = target,
            Op::Branch { target: t, .. } => *t = target,
            Op::For(f) => f.exit = target,
            Op::Each(e) => e.exit = target,
            Op::Case { next, .. } => *next = target,
            Op::OnError(ErrMode::GoTo(t)) => *t = target,
            Op::Resume(ResumeTo::Label(t)) => *t = target,
            _ => {}
        }
    }

    fn block(&mut self, body: Vec<Stmt>) {
        for s in body {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: Stmt) {
        let line = s.line;
        match s.kind {
            StmtKind::Dim { decls, is_static } => {
                for d in decls {
                    let list = if is_static {
                        &mut self.statics
                    } else {
                        &mut self.locals
                    };
                    if !list.iter().any(|x| x.name.key == d.name.key) {
                        list.push(d);
                    }
                }
            }
            StmtKind::Const(cs) => self.consts.extend(cs),
            StmtKind::Label(name) => {
                if let Ok(n) = name.parse::<u32>() {
                    self.erl_now = n;
                }
                self.labels.insert(name, self.here());
            }
            StmtKind::If { arms, otherwise } => {
                let mut ends = Vec::new();
                for arm in arms {
                    let br = self.emit(
                        Op::Branch {
                            cond: arm.cond,
                            when: false,
                            target: UNSET,
                        },
                        line,
                    );
                    self.block(arm.body);
                    ends.push(self.emit(Op::Jump(UNSET), line));
                    let next = self.here();
                    self.patch(br, next);
                }
                if let Some(o) = otherwise {
                    self.block(o);
                }
                let end = self.here();
                for e in ends {
                    self.patch(e, end);
                }
            }
            StmtKind::Select {
                test,
                arms,
                otherwise,
            } => {
                let slot = self.slot();
                self.emit(Op::Select { test, slot }, line);
                let mut ends = Vec::new();
                for arm in arms {
                    let c = self.emit(
                        Op::Case {
                            slot,
                            clauses: arm.clauses,
                            next: UNSET,
                        },
                        line,
                    );
                    self.block(arm.body);
                    ends.push(self.emit(Op::Jump(UNSET), line));
                    let next = self.here();
                    self.patch(c, next);
                }
                if let Some(o) = otherwise {
                    self.block(o);
                }
                let end = self.here();
                for e in ends {
                    self.patch(e, end);
                }
            }
            StmtKind::For {
                var,
                from,
                to,
                step,
                body,
            } => {
                let slot = self.slot();
                let init = self.emit(
                    Op::For(Box::new(ForOp {
                        var: var.clone(),
                        from,
                        to,
                        step,
                        slot,
                        exit: UNSET,
                    })),
                    line,
                );
                self.loops.push((ExitKind::For, Vec::new()));
                let start = self.here();
                self.block(body);
                self.emit(
                    Op::Next(Box::new(NextOp {
                        var,
                        slot,
                        body: start,
                    })),
                    line,
                );
                self.close_loop(Some(init));
            }
            StmtKind::ForEach {
                var,
                collection,
                body,
            } => {
                let slot = self.slot();
                let init = self.emit(
                    Op::Each(Box::new(EachOp {
                        var: var.clone(),
                        collection,
                        slot,
                        exit: UNSET,
                    })),
                    line,
                );
                self.loops.push((ExitKind::For, Vec::new()));
                let start = self.here();
                self.block(body);
                self.emit(
                    Op::EachNext(Box::new(NextOp {
                        var,
                        slot,
                        body: start,
                    })),
                    line,
                );
                self.close_loop(Some(init));
            }
            StmtKind::Do { kind, cond, body } => {
                let top = self.here();
                let pre = match (kind, &cond) {
                    (DoKind::PreWhile | DoKind::PreUntil, Some(c)) => Some(self.emit(
                        Op::Branch {
                            cond: c.clone(),
                            when: kind == DoKind::PreUntil,
                            target: UNSET,
                        },
                        line,
                    )),
                    _ => None,
                };
                self.loops.push((ExitKind::Do, Vec::new()));
                self.block(body);
                match (kind, cond) {
                    (DoKind::PostWhile, Some(c)) => {
                        self.emit(
                            Op::Branch {
                                cond: c,
                                when: true,
                                target: top,
                            },
                            line,
                        );
                    }
                    (DoKind::PostUntil, Some(c)) => {
                        self.emit(
                            Op::Branch {
                                cond: c,
                                when: false,
                                target: top,
                            },
                            line,
                        );
                    }
                    _ => {
                        self.emit(Op::Jump(top), line);
                    }
                }
                self.close_loop(pre);
            }
            StmtKind::With {
                object,
                depth,
                body,
            } => {
                self.emit(Op::WithEnter { depth, object }, line);
                self.block(body);
                self.emit(Op::WithLeave(depth), line);
            }
            StmtKind::Exit(ExitKind::Proc) => {
                self.emit(Op::Exit, line);
            }
            StmtKind::Exit(kind) => {
                let j = self.emit(Op::Jump(UNSET), line);
                match self.loops.iter_mut().rev().find(|(k, _)| *k == kind) {
                    Some((_, exits)) => exits.push(j),
                    None => {
                        let what = if kind == ExitKind::For {
                            "Exit For not within For...Next"
                        } else {
                            "Exit Do not within Do...Loop"
                        };
                        self.ops[j] = Op::Exec(StmtKind::Invalid(what.into()));
                    }
                }
            }
            StmtKind::GoTo(label) => {
                let j = self.emit(Op::Jump(UNSET), line);
                self.fixups.push((j, label));
            }
            StmtKind::GoSub(label) => {
                let j = self.emit(Op::GoSub(UNSET), line);
                self.fixups.push((j, label));
            }
            StmtKind::Return => {
                self.emit(Op::Return, line);
            }
            StmtKind::OnJump {
                index,
                labels,
                gosub,
            } => {
                self.emit(
                    Op::OnJump {
                        index,
                        labels,
                        targets: Vec::new(),
                        gosub,
                    },
                    line,
                );
            }
            StmtKind::OnError(mode) => {
                let op = match mode {
                    OnError::ResumeNext => ErrMode::ResumeNext,
                    OnError::Disable => ErrMode::Disable,
                    OnError::Reset => ErrMode::Reset,
                    OnError::GoTo(label) => {
                        let j = self.emit(Op::OnError(ErrMode::GoTo(UNSET)), line);
                        self.fixups.push((j, label));
                        return;
                    }
                };
                self.emit(Op::OnError(op), line);
            }
            StmtKind::Resume(r) => {
                let op = match r {
                    Resume::Here => ResumeTo::Here,
                    Resume::Next => ResumeTo::Next,
                    Resume::Label(label) => {
                        let j = self.emit(Op::Resume(ResumeTo::Label(UNSET)), line);
                        self.fixups.push((j, label));
                        return;
                    }
                };
                self.emit(Op::Resume(op), line);
            }
            other => {
                self.emit(Op::Exec(other), line);
            }
        }
    }

    fn close_loop(&mut self, init: Option<usize>) {
        let exit = self.here();
        if let Some(i) = init {
            self.patch(i, exit);
        }
        if let Some((_, exits)) = self.loops.pop() {
            for e in exits {
                self.patch(e, exit);
            }
        }
    }
}

pub fn compile_proc(def: ProcDef) -> Proc {
    let mut b = Builder {
        ops: Vec::new(),
        lines: Vec::new(),
        erl: Vec::new(),
        erl_now: 0,
        labels: HashMap::new(),
        fixups: Vec::new(),
        loops: Vec::new(),
        slots: 0,
        locals: Vec::new(),
        statics: Vec::new(),
        consts: Vec::new(),
    };
    b.block(def.body);
    for (at, label) in std::mem::take(&mut b.fixups) {
        match b.labels.get(&label).copied() {
            Some(target) => b.patch(at, target),
            None => b.ops[at] = Op::Exec(StmtKind::Invalid(format!("Label not defined: {label}"))),
        }
    }
    for i in 0..b.ops.len() {
        let missing = match &mut b.ops[i] {
            Op::OnJump {
                labels, targets, ..
            } => {
                let mut missing = None;
                for l in labels.iter() {
                    match b.labels.get(l) {
                        Some(&t) => targets.push(t),
                        None => {
                            missing = Some(l.clone());
                            break;
                        }
                    }
                }
                missing
            }
            _ => None,
        };
        if let Some(label) = missing {
            b.ops[i] = Op::Exec(StmtKind::Invalid(format!("Label not defined: {label}")));
        }
    }
    Proc {
        name: def.name,
        kind: def.kind,
        public: def.public,
        is_static: def.is_static,
        params: def.params,
        ret: def.ret,
        ops: b.ops,
        lines: b.lines,
        erl: b.erl,
        locals: b.locals,
        statics: b.statics,
        consts: b.consts,
        slots: b.slots,
        line: def.line,
    }
}
