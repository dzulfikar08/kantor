//! Events and the interpreter features they need: `Event`/`RaiseEvent`, `WithEvents`
//! variables (on user classes and on host objects), `Implements`, `On x GoTo/GoSub`,
//! `Application.Run` and handlers the host asks for between statements.

use std::cell::RefCell;
use std::rc::Rc;

use kantor_script::vba::{ModuleKind, Project};
use kantor_script::{
    Arg, Dispatch, Host, HostCall, NullHost, ObjectRef, ScriptError, ScriptMember, Value,
};

fn crlf(s: &str) -> String {
    s.replace('\n', "\r\n")
}

fn build(modules: &[(&str, ModuleKind, &str)]) -> Project {
    let mut p = Project::new();
    for (name, kind, src) in modules {
        p.add_module(name, *kind, &crlf(src)).unwrap();
    }
    p
}

#[test]
fn raise_event_reaches_with_events_handlers_by_ref() {
    let account = r#"
Public Event Changed(ByVal amount As Double, Cancel As Boolean)
Private total As Double

Public Sub Deposit(ByVal amount As Double)
    Dim blocked As Boolean
    RaiseEvent Changed(amount, blocked)
    If Not blocked Then total = total + amount
End Sub

Public Property Get Balance() As Double
    Balance = total
End Property
"#;
    let watcher = r#"
Private WithEvents acct As Account
Public Log As String

Public Sub Watch(a As Account)
    Set acct = a
End Sub

Private Sub acct_Changed(ByVal amount As Double, Cancel As Boolean)
    Log = Log & amount & ";"
    If amount > 100 Then Cancel = True
End Sub
"#;
    let main = r#"
Function Go() As String
    Dim a As New Account, w As New Watcher
    w.Watch a
    a.Deposit 10
    a.Deposit 500
    a.Deposit 5
    Go = w.Log & " " & a.Balance
End Function
"#;
    let mut p = build(&[
        ("Account", ModuleKind::Class, account),
        ("Watcher", ModuleKind::Class, watcher),
        ("Module1", ModuleKind::Standard, main),
    ]);
    let v = p.run("Go", &[], &mut NullHost).unwrap();
    assert_eq!(v, Value::from("10;500;5; 15"));
}

#[test]
fn raise_event_of_an_undeclared_event_is_a_compile_error() {
    let cls = "Public Sub Fire()\n    RaiseEvent Nope\nEnd Sub\n";
    let main = "Sub Go()\n    Dim c As New C1\n    c.Fire\nEnd Sub\n";
    let mut p = build(&[
        ("C1", ModuleKind::Class, cls),
        ("Module1", ModuleKind::Standard, main),
    ]);
    let e = p.run("Go", &[], &mut NullHost).unwrap_err();
    assert!(e.description.contains("Event not found"), "{e}");
}

#[test]
fn implements_dispatches_through_the_interface() {
    let shape = "Public Function Area() As Double\nEnd Function\nPublic Property Get Name() As String\nEnd Property\n";
    let circle = r#"
Implements IShape
Public R As Double

Private Function IShape_Area() As Double
    IShape_Area = 3 * R * R
End Function

Private Property Get IShape_Name() As String
    IShape_Name = "circle"
End Property

Public Function Area() As Double
    Area = -1
End Function
"#;
    let square = r#"
Implements IShape
Public S As Double

Private Function IShape_Area() As Double
    IShape_Area = S * S
End Function

Private Property Get IShape_Name() As String
    IShape_Name = "square"
End Property
"#;
    let main = r#"
Function Total() As String
    Dim shapes As New Collection, sh As IShape, c As New Circle, q As New Square, t As Double, n As String
    c.R = 2
    q.S = 3
    shapes.Add c
    shapes.Add q
    For Each sh In shapes
        t = t + sh.Area
        n = n & sh.Name & ","
    Next
    Total = n & t & "," & c.Area & "," & (TypeOf c Is IShape) & "," & Describe(q)
End Function

Function Describe(s As IShape) As String
    Describe = s.Name
End Function
"#;
    let mut p = build(&[
        ("IShape", ModuleKind::Class, shape),
        ("Circle", ModuleKind::Class, circle),
        ("Square", ModuleKind::Class, square),
        ("Module1", ModuleKind::Standard, main),
    ]);
    let v = p.run("Total", &[], &mut NullHost).unwrap();
    assert_eq!(v, Value::from("circle,square,21,-1,True,square"));
}

#[test]
fn on_goto_and_on_gosub_lists() {
    let src = r#"
Function Pick(n As Integer) As String
    Dim s As String
    On n GoTo One, Two, Three
    Pick = "none"
    Exit Function
One:
    Pick = "one": Exit Function
Two:
    Pick = "two": Exit Function
Three:
    Pick = "three"
End Function

Function Sub2(n As Integer) As String
    Dim s As String
    On n GoSub A, B
    s = s & "|end"
    Sub2 = s
    Exit Function
A:
    s = s & "a"
    Return
B:
    s = s & "b"
    Return
End Function
"#;
    let mut p = build(&[("Module1", ModuleKind::Standard, src)]);
    let pick = |p: &mut Project, n: i16| p.run("Pick", &[Value::Integer(n)], &mut NullHost);
    assert_eq!(pick(&mut p, 1).unwrap(), Value::from("one"));
    assert_eq!(pick(&mut p, 3).unwrap(), Value::from("three"));
    assert_eq!(pick(&mut p, 0).unwrap(), Value::from("none"));
    assert_eq!(pick(&mut p, 7).unwrap(), Value::from("none"));
    assert_eq!(pick(&mut p, -1).unwrap_err().number, 5);
    let v = p.run("Sub2", &[Value::Integer(2)], &mut NullHost).unwrap();
    assert_eq!(v, Value::from("b|end"));
    let v = p.run("Sub2", &[Value::Integer(0)], &mut NullHost).unwrap();
    assert_eq!(v, Value::from("|end"));
}

#[test]
fn module_variables_persist_until_end() {
    let src = r#"
Dim counter As Long
Function Bump() As Long
    counter = counter + 1
    Bump = counter
End Function
Sub Halt()
    End
End Sub
"#;
    let mut p = build(&[("Module1", ModuleKind::Standard, src)]);
    assert_eq!(p.run("Bump", &[], &mut NullHost).unwrap(), Value::Long(1));
    assert_eq!(p.run("Bump", &[], &mut NullHost).unwrap(), Value::Long(2));
    p.run("Halt", &[], &mut NullHost).unwrap();
    assert_eq!(p.run("Bump", &[], &mut NullHost).unwrap(), Value::Long(1));
}

#[test]
fn run_by_ref_returns_cancel() {
    let doc = r#"
Private Sub Worksheet_BeforeDoubleClick(ByVal Target As Variant, Cancel As Boolean)
    If Target = "locked" Then Cancel = True
End Sub
"#;
    let mut p = build(&[("Sheet1", ModuleKind::Document, doc)]);
    assert!(p.has_procedure("Sheet1.Worksheet_BeforeDoubleClick"));
    assert!(!p.has_procedure("Sheet1.Worksheet_Change"));
    let mut args = [Value::from("locked"), Value::Boolean(false)];
    p.run_by_ref(
        "Sheet1.Worksheet_BeforeDoubleClick",
        &mut args,
        &mut NullHost,
    )
    .unwrap();
    assert_eq!(args[1], Value::Boolean(true));
    let mut args = [Value::from("open"), Value::Boolean(false)];
    p.run_by_ref(
        "Sheet1.Worksheet_BeforeDoubleClick",
        &mut args,
        &mut NullHost,
    )
    .unwrap();
    assert_eq!(args[1], Value::Boolean(false));
}

/// A host whose globals are an `Application` with `Run`, and which fires a handler after the
/// first statement that writes `Changed`.
struct AppHost {
    app: ObjectRef,
    pending: Rc<RefCell<Vec<HostCall>>>,
}

struct App {
    pending: Rc<RefCell<Vec<HostCall>>>,
}

impl Dispatch for App {
    fn type_name(&self) -> String {
        "Application".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "application" => Ok(Value::object(Rc::new(App {
                pending: self.pending.clone(),
            }))),
            _ => Err(ScriptError::no_member("Application", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "changed" => {
                self.pending.borrow_mut().push(HostCall::Proc {
                    name: "Sheet1.Worksheet_Change".into(),
                    args: vec![value.clone()],
                });
                self.pending.borrow_mut().push(HostCall::Event {
                    source_type: "Application".into(),
                    event: "SheetChange".into(),
                    args: vec![value],
                });
                Ok(())
            }
            _ => Err(ScriptError::no_member("Application", name)),
        }
    }

    fn script_member(&self, name: &str) -> Option<ScriptMember> {
        name.eq_ignore_ascii_case("run")
            .then_some(ScriptMember::Run)
    }
}

impl Host for AppHost {
    fn globals(&mut self) -> Option<ObjectRef> {
        Some(self.app.clone())
    }

    fn take_calls(&mut self) -> Vec<HostCall> {
        std::mem::take(&mut *self.pending.borrow_mut())
    }
}

#[test]
fn application_run_host_calls_and_application_events() {
    let module = r#"
Public Log As String

Function Twice(x)
    Twice = x * 2
End Function

Function Go() As String
    Dim r
    r = Application.Run("Twice", 21)
    r = r + Run("'Book1.xlsm'!Module1.Twice", 1)
    Application.Changed = "A1"
    Log = Log & "after;"
    Go = r & ":" & Log
End Function
"#;
    let sheet = r#"
Private Sub Worksheet_Change(ByVal Target As Variant)
    Log = Log & "sheet " & Target & ";"
End Sub
"#;
    let this = r#"
Private WithEvents App As Application

Public Sub Hook()
    Set App = Application
End Sub

Private Sub App_SheetChange(ByVal Target As Variant)
    Log = Log & "app " & Target & ";"
End Sub
"#;
    let mut p = build(&[
        ("Module1", ModuleKind::Standard, module),
        ("Sheet1", ModuleKind::Document, sheet),
        ("ThisWorkbook", ModuleKind::Document, this),
    ]);
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut host = AppHost {
        app: Rc::new(App {
            pending: pending.clone(),
        }),
        pending,
    };
    assert!(!p.has_event_sinks());
    p.run("ThisWorkbook.Hook", &[], &mut host).unwrap();
    assert!(p.has_event_sinks());
    let v = p.run("Go", &[], &mut host).unwrap();
    assert_eq!(v, Value::from("44:sheet A1;app A1;after;"));
    // Raised by the host directly (a user edit).
    let mut args = [Value::from("B2")];
    p.raise_event("Application", "SheetChange", &mut args, &mut host)
        .unwrap();
    let log = p.run("Module1.Go", &[], &mut host).unwrap();
    assert!(format!("{log:?}").contains("app B2;"), "{log:?}");
}

#[test]
fn errors_in_host_called_handlers_are_not_trapped_by_the_caller() {
    let module = r#"
Function Go() As String
    On Error Resume Next
    Application.Changed = "A1"
    Go = "kept going"
End Function
"#;
    let sheet =
        "Private Sub Worksheet_Change(ByVal Target As Variant)\n    Err.Raise 1004\nEnd Sub\n";
    let mut p = build(&[
        ("Module1", ModuleKind::Standard, module),
        ("Sheet1", ModuleKind::Document, sheet),
    ]);
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut host = AppHost {
        app: Rc::new(App {
            pending: pending.clone(),
        }),
        pending,
    };
    let e = p.run("Go", &[], &mut host).unwrap_err();
    assert_eq!(e.number, 1004);
    assert_eq!(e.location.unwrap().procedure, "Worksheet_Change");
}
