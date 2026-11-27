//! Realistic office macros run against a mock spreadsheet host.

mod common;

use common::{MockHost, project, run, run_str};
use kantor_script::vba::{ModuleKind, ProcKind, Project};
use kantor_script::{ErrorKind, Value};

#[test]
fn fills_rows_through_cells_and_range() {
    let src = r##"
Option Explicit
Sub FillRows()
    Dim r As Long, total As Double
    For r = 1 To 10
        Cells(r, 1).Value = r
        Cells(r, 2).Value = r * r
        total = total + Cells(r, 2).Value
    Next r
    Range("C1").Value = total
    Range("D1") = "Total: " & Format(total, "#,##0.00")
    Range("E1:E3").Value = 7
    Range("A1").Copy Destination:=Range("F1")
    ActiveSheet.Range("G1").Value = ActiveSheet.Name
    Range("H1") = WorksheetFunction.Sum(Range("B1:B10"))
End Sub
"##;
    let (_, host) = run(src, "FillRows");
    assert_eq!(host.cell("A10"), Value::Long(10));
    assert_eq!(host.cell("B10"), Value::Long(100));
    assert_eq!(host.cell("C1"), Value::Double(385.0));
    assert_eq!(host.cell("D1"), Value::from("Total: 385.00"));
    assert_eq!(host.cell("E3"), Value::Integer(7));
    assert_eq!(host.cell("F1"), Value::Long(1));
    assert_eq!(host.cell("G1"), Value::from("Sheet1"));
    assert_eq!(host.cell("H1"), Value::Double(385.0));
}

#[test]
fn crlf_lf_and_cr_sources_behave_the_same() {
    let src = "Function F() As String\n    Dim i As Integer, s As String\n    For i = 1 To 3: s = s & i: Next\n    F = s & _\n        \"!\"\nEnd Function\n";
    for variant in [
        src.to_string(),
        src.replace('\n', "\r\n"),
        src.replace('\n', "\r"),
    ] {
        assert_eq!(run_str(&variant, "F"), "123!");
    }
}

#[test]
fn walks_a_column_until_empty() {
    let src = r##"
Function JoinColumn(col As Long) As String
    Dim r As Long, parts As String
    r = 1
    Do While Cells(r, col).Value <> ""
        If parts <> "" Then parts = parts & ", "
        parts = parts & Cells(r, col).Value
        r = r + 1
    Loop
    JoinColumn = parts
End Function
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    host.set("A1", "apple");
    host.set("A2", "banana");
    host.set("A3", 3.0);
    let v = p
        .call_function("JoinColumn", &[Value::Long(1)], &mut host)
        .unwrap();
    assert_eq!(v, Value::from("apple, banana, 3"));
}

#[test]
fn error_handling_paths() {
    let src = r##"
Function SafeDivide(a As Double, b As Double) As Variant
    On Error GoTo Fail
    SafeDivide = a / b
    Exit Function
Fail:
    SafeDivide = "Error " & Err.Number & ": " & Err.Description
End Function

Function ResumeNextDemo() As String
    Dim s As String
    On Error Resume Next
    s = "a"
    Err.Raise 1000, "Demo", "custom failure"
    s = s & "|" & Err.Number & "|" & Err.Description & "|" & Err.Source
    Err.Clear
    s = s & "|" & Err.Number
    ResumeNextDemo = s
End Function

Function Retry() As Long
    Dim d As Long
    On Error GoTo Handler
    Retry = 100 \ d
    Exit Function
Handler:
    d = 4
    Resume
End Function

Function ResumeAtLabel() As String
    On Error GoTo Handler
    Err.Raise 13
    ResumeAtLabel = "not here"
    Exit Function
Handler:
    Resume Done
Done:
    ResumeAtLabel = "resumed after " & Err.Number
End Function

Sub Inner()
    Dim x As Integer
    x = 40000
End Sub

Function Caller() As String
    On Error GoTo H
    Inner
    Caller = "no error"
    Exit Function
H:
    Caller = "caught " & Err.Number & " in caller"
End Function

Function ErrorInIf() As String
    Dim s As String
    On Error Resume Next
    If 1 / 0 > 0 Then
        s = "then-branch"
    Else
        s = "else-branch"
    End If
    ErrorInIf = s
End Function
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    let mut call = |name: &str, args: &[Value]| p.call_function(name, args, &mut host).unwrap();
    assert_eq!(
        call("SafeDivide", &[Value::Long(1), Value::Long(4)]),
        Value::Double(0.25)
    );
    assert_eq!(
        call("SafeDivide", &[Value::Long(1), Value::Long(0)]),
        Value::from("Error 11: Division by zero")
    );
    assert_eq!(
        call("ResumeNextDemo", &[]),
        Value::from("a|1000|custom failure|Demo|0")
    );
    assert_eq!(call("Retry", &[]), Value::Long(25));
    // `Resume label` clears Err (VBA reference, "Resume statement").
    assert_eq!(call("ResumeAtLabel", &[]), Value::from("resumed after 0"));
    assert_eq!(call("Caller", &[]), Value::from("caught 6 in caller"));
    // Resume Next continues with the statement after the failing one: for a failing If
    // condition that's the first statement of the Then branch, as in VBA.
    assert_eq!(call("ErrorInIf", &[]), Value::from("then-branch"));
}

#[test]
fn unhandled_errors_carry_module_procedure_and_line() {
    let src = "Sub Outer()\n    Inner\nEnd Sub\nSub Inner()\n    Dim x As Integer\n    x = 40000\nEnd Sub\n";
    let mut p = project(src);
    let err = p.run("Outer", &[], &mut MockHost::new()).unwrap_err();
    assert_eq!(err.number, 6);
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert!(err.to_string().contains("Overflow"), "{err}");
    let loc = err.location.expect("location");
    assert_eq!(
        (loc.module.as_str(), loc.procedure.as_str(), loc.line),
        ("Module1", "Inner", 6)
    );
}

#[test]
fn byref_and_byval_semantics() {
    let src = r##"
Sub Swap(a, b)
    Dim t
    t = a: a = b: b = t
End Sub

Sub Bump(ByVal n As Long, ByRef m As Long)
    n = n + 1
    m = m + 1
End Sub

Function Demo() As String
    Dim x As Long, y As Long, arr(1 To 3) As String, p As Long, q As Long
    x = 1: y = 2
    Swap x, y
    arr(1) = "one": arr(3) = "three"
    Swap arr(1), arr(3)
    p = 10: q = 20
    Bump p, q
    Bump (p), (q)
    Call Bump(p, q)
    Demo = x & y & "|" & arr(1) & arr(3) & "|" & p & "," & q
End Function
"##;
    // MS-VBAL §5.3.1.11: a variable argument is passed by reference, a parenthesised one by
    // value; array elements are passed by reference too.
    assert_eq!(run_str(src, "Demo"), "21|threeone|10,22");
}

#[test]
fn user_defined_types_with_and_byref_fields() {
    let src = r##"
Type Employee
    Name As String
    Salary As Currency
    Skills(1 To 3) As String
End Type

Sub Raise10(ByRef amount As Currency)
    amount = amount * 1.1
End Sub

Function UdtDemo() As String
    Dim e As Employee
    With e
        .Name = "Sari"
        .Salary = 1000
        .Skills(1) = "VBA"
    End With
    Raise10 e.Salary
    Dim copy As Employee
    copy = e
    copy.Name = "Budi"
    UdtDemo = e.Name & " " & e.Salary & " " & e.Skills(1) & " " & copy.Name & " " & TypeName(e)
End Function
"##;
    assert_eq!(run_str(src, "UdtDemo"), "Sari 1100 VBA Budi Employee");
}

const ACCOUNT: &str = r##"
Option Explicit
Private m_balance As Currency
Public Owner As String
Private m_log As Collection

Private Sub Class_Initialize()
    Set m_log = New Collection
    m_balance = 0
End Sub

Private Sub Class_Terminate()
    Debug.Print "closing " & Owner
End Sub

Public Property Get Balance() As Currency
    Balance = m_balance
End Property

Public Property Let Balance(ByVal v As Currency)
    If v < 0 Then Err.Raise vbObjectError + 1, "Account", "Negative balance"
    m_balance = v
End Property

Public Sub Deposit(ByVal amount As Currency)
    Balance = Balance + amount
    m_log.Add "deposit " & amount
End Sub

Public Function History() As String
    Dim item As Variant, s As String
    For Each item In m_log
        s = s & item & ";"
    Next
    History = s
End Function
"##;

#[test]
fn classes_properties_collections() {
    let main = r##"
Function Bank() As String
    Dim a As Account, b As New Account
    Set a = New Account
    a.Owner = "Ana"
    a.Deposit 100
    a.Deposit 50.5
    b.Owner = "Ben"
    b.Balance = 20
    Dim accounts As New Collection
    accounts.Add a, "ana"
    accounts.Add b, "ben"
    Dim total As Currency, acc As Account
    For Each acc In accounts
        total = total + acc.Balance
    Next
    On Error Resume Next
    b.Balance = -5
    Dim msg As String
    msg = Err.Number - vbObjectError & " " & Err.Description
    On Error GoTo 0
    Bank = total & "|" & accounts("ana").History() & "|" & accounts.Count & "|" & msg & "|" & TypeName(a) & "|" & (TypeOf a Is Account)
    Set acc = Nothing
    accounts.Remove "ana"
    Set a = Nothing
End Function
"##;
    let mut p = Project::new();
    p.add_module("Account", ModuleKind::Class, ACCOUNT).unwrap();
    p.add_module("Module1", ModuleKind::Standard, main).unwrap();
    let mut host = MockHost::new();
    let v = p.run("Bank", &[], &mut host).unwrap();
    assert_eq!(
        v,
        Value::from("170.5|deposit 100;deposit 50.5;|2|1 Negative balance|Account|True")
    );
    // Class_Terminate runs when the last reference goes away.
    assert!(
        host.printed.contains(&"closing Ana".to_string()),
        "{:?}",
        host.printed
    );
    // Class methods aren't macros.
    assert!(p.run("Account.Deposit", &[], &mut host).is_err());
}

#[test]
fn custom_collection_class_with_default_member_and_enumerator() {
    let class = r##"
Private items As New Collection
Public Sub Add(v)
    items.Add v
End Sub
Public Property Get Item(i)
Attribute Item.VB_UserMemId = 0
    Item = items(i)
End Property
Public Function NewEnum()
Attribute NewEnum.VB_UserMemId = -4
    Set NewEnum = items
End Function
"##;
    let main = r##"
Function UseList() As String
    Dim l As New List, v, s As String
    l.Add "x": l.Add "y"
    For Each v In l
        s = s & v
    Next
    UseList = s & l(2)
End Function
"##;
    let mut p = Project::new();
    p.add_module("List", ModuleKind::Class, class).unwrap();
    p.add_module("Module1", ModuleKind::Standard, main).unwrap();
    assert_eq!(
        p.run("UseList", &[], &mut MockHost::new()).unwrap(),
        Value::from("xyy")
    );
}

#[test]
fn scripting_dictionary_word_count() {
    let src = r##"
Function WordCount(text As String) As String
    Dim d As Object, w As Variant, k As Variant, out As String
    Set d = CreateObject("Scripting.Dictionary")
    d.CompareMode = vbTextCompare
    For Each w In Split(text, " ")
        If w <> "" Then
            If d.Exists(w) Then
                d(w) = d(w) + 1
            Else
                d.Add w, 1
            End If
        End If
    Next
    For Each k In d.Keys
        out = out & k & "=" & d(k) & " "
    Next
    d.Remove "dog"
    WordCount = Trim(out) & " (" & d.Count & ")"
End Function
"##;
    let mut p = project(src);
    let v = p
        .call_function(
            "WordCount",
            &[Value::from("the cat The dog  cat the")],
            &mut MockHost::new(),
        )
        .unwrap();
    assert_eq!(v, Value::from("the=3 cat=2 dog=1 (2)"));
}

#[test]
fn select_case_forms() {
    let src = r##"
Function Grade(score As Variant) As String
    Select Case score
        Case "absent", "sick": Grade = "excused"
        Case Is >= 90: Grade = "A"
        Case 80 To 89
            Grade = "B"
        Case 70, 75, 79: Grade = "C"
        Case Else: Grade = "F"
    End Select
End Function

Function Bucket(s As String) As String
    Select Case LCase(s)
        Case "a" To "m": Bucket = "first half"
        Case Else: Bucket = "second half"
    End Select
End Function
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    let mut g = |v: Value| p.call_function("Grade", &[v], &mut host).unwrap();
    assert_eq!(g(Value::Long(95)), Value::from("A"));
    assert_eq!(g(Value::Long(85)), Value::from("B"));
    assert_eq!(g(Value::Long(75)), Value::from("C"));
    assert_eq!(g(Value::Long(72)), Value::from("F"));
    assert_eq!(g(Value::from("absent")), Value::from("excused"));
    let mut p = project(src);
    assert_eq!(
        p.call_function("Bucket", &[Value::from("Kiwi")], &mut host)
            .unwrap(),
        Value::from("first half")
    );
    assert_eq!(
        p.call_function("Bucket", &[Value::from("pear")], &mut host)
            .unwrap(),
        Value::from("second half")
    );
}

#[test]
fn nested_with_blocks() {
    let src = r##"
Sub Report()
    With Range("A1")
        .Value = "Header"
        With .Offset(1, 0)
            .Value = 42
        End With
        .Offset(2, 0).Value = .Value & "!"
    End With
    With Application
        .ScreenUpdating = False
    End With
End Sub
"##;
    let (_, host) = run(src, "Report");
    assert_eq!(host.cell("A1"), Value::from("Header"));
    assert_eq!(host.cell("A2"), Value::Integer(42));
    assert_eq!(host.cell("A3"), Value::from("Header!"));
    assert!(!*host.app.screen_updating.borrow());
}

#[test]
fn date_math() {
    let src = r##"
Function Dates() As String
    Dim d As Date, s As String
    d = DateSerial(2024, 1, 31)
    s = Format(DateAdd("m", 1, d), "yyyy-mm-dd")
    s = s & " " & DateDiff("d", #1/1/2024#, #3/1/2024#)
    s = s & " " & DateDiff("m", #12/31/2023#, #1/1/2024#)
    s = s & " " & Format(DateSerial(2024, 13, 1), "yyyy-mm-dd")
    s = s & " " & Weekday(#3/15/2024#)
    s = s & " " & WeekdayName(Weekday(#3/15/2024#))
    s = s & " " & MonthName(2, True)
    s = s & " " & Year(d) & "/" & Month(d) & "/" & Day(d)
    s = s & " " & Format(d + 1.5, "dd mmm yyyy hh:nn")
    s = s & " " & DatePart("q", d)
    s = s & " " & DateDiff("h", #1/1/2024 10:59:00 PM#, #1/2/2024#)
    s = s & " " & Format(Now, "yyyy-mm-dd hh:nn") & " " & (Date = #3/15/2024#)
    s = s & " " & CDate("2024-02-10") - CDate("2024-02-01")
    s = s & " " & IsDate("31/12/2023") & " " & IsDate("hello")
    Dates = s
End Function
"##;
    assert_eq!(
        run_str(src, "Dates"),
        "2024-02-29 60 1 2025-01-01 6 Friday Feb 2024/1/31 01 Feb 2024 12:00 1 2 2024-03-15 14:30 True 9 True False"
    );
}

#[test]
fn conversions_operators_and_rounding() {
    let src = r##"
Function Conv() As String
    Dim s As String
    s = CInt(2.5) & " " & CInt(3.5) & " " & CLng(-2.5) & " " & Round(2.5) & " " & Round(1.2345, 2) & " " & Int(-2.5) & " " & Fix(-2.5)
    s = s & " " & (7 \ 2) & " " & (-7 Mod 3) & " " & (2 ^ 10) & " " & (-2 ^ 2) & " " & (5 / 2)
    s = s & " " & ("3" + 4) & " " & ("3" & 4) & " " & (True + 1) & " " & Hex(255) & " " & Hex(-1) & " " & Oct(8)
    s = s & " " & Val("12.5kg") & " " & CStr(1 / 3) & " " & Str(42) & "|"
    s = s & (12 And 10) & " " & (12 Or 3) & " " & (Not 0) & " " & (True Xor False) & " " & &HFF & " " & (1 < 2 = True)
    Conv = s
End Function

Function Overflow() As String
    Dim i As Integer
    On Error Resume Next
    i = 32767
    i = i + 1
    Overflow = Err.Number & " " & i
End Function

Function Nulls() As String
    Nulls = IsNull(Null + 1) & " " & IsNull(Len(Null)) & " " & IsNull(Null & "") & " " & (Null And False)
End Function
"##;
    assert_eq!(
        run_str(src, "Conv"),
        "2 4 -2 2 1.23 -3 -2 3 -1 1024 -4 2.5 7 34 0 FF FFFF 10 12.5 0.333333333333333  42|8 15 -1 True 255 True"
    );
    assert_eq!(run_str(src, "Overflow"), "6 32767");
    assert_eq!(run_str(src, "Nulls"), "True True False False");
}

#[test]
fn option_compare_and_like() {
    let binary = r##"
Function Cmp() As String
    Cmp = ("abc" = "ABC") & " " & InStr(1, "Hello", "LL") & " " & InStr(1, "Hello", "LL", vbTextCompare) & " " & ("F" Like "[A-Z]") & " " & ("abc" < "abd") & " " & ("a1" Like "[a-z]#")
End Function
"##;
    let text = r##"
Option Compare Text
Function TextCmp() As String
    TextCmp = ("abc" = "ABC") & " " & ("Apple" Like "a*") & " " & InStr("Hello", "LL") & " " & StrComp("a", "B")
End Function
"##;
    assert_eq!(run_str(binary, "Cmp"), "False 0 3 True True True");
    assert_eq!(run_str(text, "TextCmp"), "True True 3 -1");
}

#[test]
fn statics_and_module_variables_persist_until_reset() {
    let src = r##"
Private counter As Long
Function NextId() As Long
    Static calls As Long
    calls = calls + 1
    counter = counter + 10
    NextId = calls * 1000 + counter
End Function
Sub Finish()
    End
End Sub
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    let mut next = |p: &mut Project| p.call_function("NextId", &[], &mut host).unwrap();
    assert_eq!(next(&mut p), Value::Long(1010));
    assert_eq!(next(&mut p), Value::Long(2020));
    p.reset();
    assert_eq!(next(&mut p), Value::Long(1010));
    // `End` stops the run and resets module variables.
    p.run("Finish", &[], &mut MockHost::new()).unwrap();
    assert_eq!(next(&mut p), Value::Long(1010));
}

#[test]
fn arrays_redim_erase_and_for_each() {
    let src = r##"
Function Arrays() As String
    Dim a() As Long, i As Long, s As String, m(1 To 2, 1 To 3) As Integer, v As Variant, total As Long
    ReDim a(1 To 3)
    For i = 1 To 3: a(i) = i * 10: Next i
    ReDim Preserve a(1 To 5)
    a(5) = 50
    s = LBound(a) & "-" & UBound(a) & ":" & a(3) & "," & a(4) & "," & a(5)
    For i = 1 To 3
        m(1, i) = i
        m(2, i) = i * 100
    Next
    For Each v In m
        total = total + v
    Next
    s = s & "|" & UBound(m, 2) & "|" & total
    v = Array("x", "y", "z")
    s = s & "|" & Join(v, "+") & "|" & UBound(v) & "|" & TypeName(Split("a,b", ","))
    Erase a
    On Error Resume Next
    i = UBound(a)
    s = s & "|" & Err.Number
    Err.Clear
    Dim fixedArr(2) As Long
    ReDim fixedArr(5)
    s = s & "|" & Err.Number
    Arrays = s
End Function

Function Grid() As Variant
    Dim data As Variant, r As Long, c As Long
    data = Range("A1:B2").Value
    For r = 1 To 2
        For c = 1 To 2
            data(r, c) = data(r, c) * 10
        Next c, r
    Range("C1:D2").Value = data
    Grid = UBound(data, 1) & "x" & UBound(data, 2)
End Function
"##;
    assert_eq!(
        run_str(src, "Arrays"),
        "1-5:30,0,50|3|606|x+y+z|2|String()|9|10"
    );
    let mut p = project(src);
    let mut host = MockHost::new();
    host.set("A1", 1.0);
    host.set("B1", 2.0);
    host.set("A2", 3.0);
    host.set("B2", 4.0);
    assert_eq!(p.run("Grid", &[], &mut host).unwrap(), Value::from("2x2"));
    assert_eq!(host.cell("D2"), Value::Double(40.0));
    assert_eq!(host.cell("C2"), Value::Double(30.0));
}

#[test]
fn procedures_optional_paramarray_recursion() {
    let src = r##"
Function Fact(n As Long) As Double
    If n <= 1 Then
        Fact = 1
    Else
        Fact = n * Fact(n - 1)
    End If
End Function

Function Greet(Optional name As Variant, Optional greeting As String = "Hello") As String
    If IsMissing(name) Then name = "world"
    Greet = greeting & ", " & name
End Function

Function SumAll(ParamArray nums() As Variant) As Double
    Dim x As Variant
    For Each x In nums
        SumAll = SumAll + x
    Next
End Function

Function UseThem() As String
    UseThem = Fact(10) & "|" & Greet() & "|" & Greet("Ana") & "|" & Greet(greeting:="Hi", name:="Bo") & "|" & SumAll(1, 2, 3.5) & "|" & SumAll()
End Function

Private Sub Helper()
End Sub
"##;
    assert_eq!(
        run_str(src, "UseThem"),
        "3628800|Hello, world|Hello, Ana|Hi, Bo|6.5|0"
    );
    let procs = project(src).procedures();
    let names: Vec<_> = procs
        .iter()
        .map(|p| (p.name.as_str(), p.kind, p.is_macro))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Fact", ProcKind::Function, false),
            ("Greet", ProcKind::Function, false),
            ("SumAll", ProcKind::Function, false),
            ("UseThem", ProcKind::Function, false),
            ("Helper", ProcKind::Sub, false),
        ]
    );
}

#[test]
fn enums_constants_and_host_constants() {
    let src = r##"
Public Enum Level
    Low = 1
    Medium
    High = 10
End Enum
Public Const APP_NAME As String = "Kantor"
Private Const LIMIT = High * 2

Function EnumDemo() As String
    Dim lv As Level, answer As VbMsgBoxResult
    lv = Medium
    answer = vbYes
    EnumDemo = APP_NAME & " " & lv & " " & Level.High & " " & LIMIT & " " & (lv = 2) & " " & answer & " " & xlUp & " " & MyConst
End Function
"##;
    let mut p = project(src);
    p.register_constant("MyConst", Value::from("registered"));
    let v = p.run("EnumDemo", &[], &mut MockHost::new()).unwrap();
    assert_eq!(v, Value::from("Kantor 2 10 20 True 6 -4162 registered"));
}

#[test]
fn goto_gosub_and_labels() {
    let src = r##"
Function Gs() As String
    Dim s As String, i As Integer
    s = "a"
    GoSub AddB
    s = s & "c"
    GoTo Done
AddB:
    s = s & "b"
    Return
Done:
    i = 0
10  i = i + 1
    If i < 3 Then GoTo 10
    Gs = s & i
End Function
"##;
    assert_eq!(run_str(src, "Gs"), "abc3");
}

#[test]
fn debug_print_output() {
    let src = r##"
Sub Printing()
    Debug.Print "a"; "b"
    Debug.Print 1; -2; "x"
    Debug.Print "col1", "col2"
    Debug.Print "partial";
    Debug.Print " rest"
    Debug.Print
    Debug.Print Null; True; #1/2/2024#
End Sub
"##;
    let (_, host) = run(src, "Printing");
    assert_eq!(
        host.printed,
        vec![
            "ab",
            " 1 -2 x",
            "col1          col2",
            "partial rest",
            "",
            "NullTrue1/2/2024"
        ]
    );
}

#[test]
fn msgbox_and_inputbox_go_through_the_host() {
    let src = r##"
Function Ask() As String
    Dim who As String, answer As VbMsgBoxResult
    who = InputBox("Your name?", "Hello", "anon")
    answer = MsgBox("Save " & who & "?", vbYesNo + vbQuestion, "Confirm")
    If answer = vbYes Then Ask = "saved " & who Else Ask = "skipped " & who
    MsgBox Prompt:="Done", Buttons:=vbOKOnly
End Function
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    host.inputs.push_back(Some("Dewi".into()));
    host.answers.push_back(6);
    assert_eq!(
        p.run("Ask", &[], &mut host).unwrap(),
        Value::from("saved Dewi")
    );
    assert_eq!(host.prompts, vec!["Your name?", "Save Dewi?", "Done"]);
    host.inputs.push_back(None);
    host.answers.push_back(7);
    assert_eq!(
        p.run("Ask", &[], &mut host).unwrap(),
        Value::from("skipped ")
    );
}

#[test]
fn rnd_matches_vba_sequence() {
    let src =
        "Function R() As String\n    R = Rnd() & \" \" & Rnd & \" \" & Rnd(0)\nEnd Function\n";
    // VBA's generator gives 0.7055475, 0.533424 after start-up (no Randomize).
    assert_eq!(run_str(src, "R"), "0.7055475 0.533424 0.533424");
}

#[test]
fn string_functions_and_mid_statement() {
    let src = r##"
Function Strings() As String
    Dim s As String, f As String * 5, t As String, r As String
    s = "Hello World"
    Mid(s, 1, 5) = "HOWDY"
    t = "abcdef"
    Mid(t, 3) = "XY"
    f = "abcdefgh"
    r = "    "
    RSet r = "ab"
    Strings = s & "|" & t & "|" & f & "|" & r & "|" & Len(f) & "|" & Replace("a-b-c", "-", "+") & "|" & Replace("aaa", "a", "b", 2, 1) & "|" & StrReverse("abc") & "|" & UCase(Left("hello", 1)) & Mid("hello", 2) & "|" & InStrRev("a/b/c", "/") & "|" & Space(2) & "x" & "|" & String(3, "*") & "|" & Trim("  pad  ") & "|" & Chr(65) & Asc("a") & "|" & Split("1,2,3", ",")(1) & "|" & Format(0.256, "0.0%") & "|" & StrConv("hello world", vbProperCase)
End Function
"##;
    assert_eq!(
        run_str(src, "Strings"),
        "HOWDY World|abXYef|abcde|  ab|5|a+b+c|ba|cba|Hello|4|  x|***|pad|A97|2|25.6%|Hello World"
    );
}

#[test]
fn type_information() {
    let src = r##"
Function Types() As String
    Dim c As New Collection, o As Object
    Types = TypeName(5) & " " & TypeName(70000) & " " & TypeName(1.5) & " " & TypeName("s") & " " & TypeName(Empty) & " " & TypeName(Null) & " " & TypeName(o) & " " & TypeName(c) & " " & VarType("x") & " " & VarType(1.5) & " " & IsNumeric("12.5") & " " & IsNumeric("abc") & " " & IsEmpty(Empty) & " " & IsArray(Array()) & " " & IsObject(o) & " " & (o Is Nothing) & " " & TypeName(Range("A1")) & " " & IsEmpty(Range("A1"))
End Function
"##;
    assert_eq!(
        run_str(src, "Types"),
        "Integer Long Double String Empty Null Nothing Collection 8 5 True False True True True True Range True"
    );
}

#[test]
fn option_explicit_and_implicit_variables() {
    let explicit = "Option Explicit\nFunction F()\n    y = 1\n    F = y\nEnd Function\n";
    let mut p = project(explicit);
    let err = p.run("F", &[], &mut MockHost::new()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Syntax);
    assert!(
        err.description.contains("Variable not defined: y"),
        "{}",
        err.description
    );
    let implicit = "Function F()\n    y = 1\n    F = y + z\nEnd Function\n";
    assert_eq!(run(implicit, "F").0, Value::Integer(1));
}

#[test]
fn unsupported_features_fail_clearly_without_blocking_the_module() {
    let src = r##"
Private Declare PtrSafe Function GetTickCount Lib "kernel32" () As Long
#If VBA7 Then
Private Const WIDE = True
#Else
Private Const WIDE = False
#End If

Sub UsesFiles()
    Open "C:\data.txt" For Output As #1
    Print #1, "x"
    Close #1
End Sub

Function TryShell() As String
    On Error Resume Next
    Shell "notepad.exe"
    TryShell = Err.Number & ": " & Err.Description
End Function

Function Ticks() As Long
    Ticks = GetTickCount()
End Function

Function MakeExcel() As Object
    Set MakeExcel = CreateObject("Excel.Application")
End Function

Sub Bad()
    x = = 1
End Sub

Function Fine() As Boolean
    Fine = WIDE
End Function
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    let e = p.run("UsesFiles", &[], &mut host).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.description.contains("Open"), "{}", e.description);
    assert_eq!(e.location.as_ref().map(|l| l.line), Some(10));
    assert_eq!(
        p.run("TryShell", &[], &mut host).unwrap(),
        Value::from(
            "445: Running or controlling other programs (Shell) is not supported in Kantor"
        )
    );
    let e = p.run("Ticks", &[], &mut host).unwrap_err();
    assert!(
        e.description.contains("GetTickCount") && e.description.contains("kernel32"),
        "{}",
        e.description
    );
    let e = p.run("MakeExcel", &[], &mut host).unwrap_err();
    assert_eq!(e.number, 429);
    assert!(
        e.description.contains("Excel.Application"),
        "{}",
        e.description
    );
    let e = p.run("Bad", &[], &mut host).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Syntax);
    assert_eq!(e.location.as_ref().map(|l| l.line), Some(30));
    assert_eq!(p.run("Fine", &[], &mut host).unwrap(), Value::Boolean(true));
}

#[test]
fn broken_structure_is_reported_with_its_line() {
    let mut p = Project::new();
    let err = p
        .add_module(
            "Module1",
            ModuleKind::Standard,
            "Sub A()\n    If x Then\n        y = 1\nEnd Sub\n",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Syntax);
    let loc = err.location.expect("location");
    assert_eq!((loc.module.as_str(), loc.line), ("Module1", 4));
}

#[test]
fn runaway_loops_stop() {
    let src = "Sub Forever()\n    Dim i As Long\n    Do\n        i = i + 1\n    Loop\nEnd Sub\n";
    let mut p = project(src);
    p.set_step_limit(Some(10_000));
    let e = p.run("Forever", &[], &mut MockHost::new()).unwrap_err();
    assert_eq!((e.number, e.kind), (18, ErrorKind::Interrupted));

    // A cancel flag set by the UI stops it too, and On Error can't trap it.
    let src = "Sub Forever()\n    On Error Resume Next\n    Do\n    Loop\nEnd Sub\n";
    let mut p = project(src);
    let mut host = MockHost::new();
    host.cancel.cancel();
    let e = p.run("Forever", &[], &mut host).unwrap_err();
    assert_eq!((e.number, e.kind), (18, ErrorKind::Interrupted));
}

#[test]
fn deep_recursion_is_out_of_stack_space() {
    let src = "Function Down(n As Long) As Long\n    Down = Down(n + 1)\nEnd Function\n";
    let mut p = project(src);
    p.set_max_call_depth(50);
    let e = p
        .run("Down", &[Value::Long(0)], &mut MockHost::new())
        .unwrap_err();
    assert_eq!(e.number, 28);
}

#[test]
fn udf_receives_ranges() {
    let src = r##"
Function Doubled(r As Variant) As Double
    Doubled = r.Value * 2
End Function
Function CountAbove(rng As Object, limit As Double) As Long
    Dim cell As Object
    For Each cell In rng
        If cell.Value > limit Then CountAbove = CountAbove + 1
    Next
End Function
"##;
    let mut p = project(src);
    let mut host = MockHost::new();
    host.set("A1", 4.0);
    host.set("A2", 10.0);
    host.set("A3", 12.0);
    let a1 = host.app.get_range("A1");
    let col = host.app.get_range("A1:A3");
    assert_eq!(
        p.call_function("Doubled", &[a1], &mut host).unwrap(),
        Value::Double(8.0)
    );
    assert_eq!(
        p.call_function("CountAbove", &[col, Value::Long(5)], &mut host)
            .unwrap(),
        Value::Long(2)
    );
}

#[test]
fn class_modules_from_exported_files() {
    let class = "VERSION 1.0 CLASS\r\nBEGIN\r\n  MultiUse = -1  'True\r\nEND\r\nAttribute VB_Name = \"Counter\"\r\nAttribute VB_GlobalNameSpace = False\r\nPrivate n As Long\r\nPublic Function Up() As Long\r\n    n = n + 1\r\n    Up = n\r\nEnd Function\r\n";
    let main = "Attribute VB_Name = \"Module1\"\r\nFunction Twice() As Long\r\n    Dim c As New Counter\r\n    c.Up\r\n    Twice = c.Up\r\nEnd Function\r\n";
    let mut p = Project::new();
    p.add_module("Counter", ModuleKind::Class, class).unwrap();
    p.add_module("Module1", ModuleKind::Standard, main).unwrap();
    assert_eq!(
        p.call_function("Twice", &[], &mut MockHost::new()).unwrap(),
        Value::Long(2)
    );
}

#[test]
fn qualified_calls_and_document_modules() {
    let sheet = "Public Sub Hello()\n    Range(\"A1\") = \"from sheet\"\nEnd Sub\n";
    let main = "Public Total As Long\nSub Go()\n    Sheet1.Hello\n    Module1.Total = 5\n    Range(\"A2\") = Module1.Total + VBA.Len(\"abc\")\nEnd Sub\n";
    let mut p = Project::new();
    p.add_module("Sheet1", ModuleKind::Document, sheet).unwrap();
    p.add_module("Module1", ModuleKind::Standard, main).unwrap();
    let mut host = MockHost::new();
    p.run("Go", &[], &mut host).unwrap();
    assert_eq!(host.cell("A1"), Value::from("from sheet"));
    assert_eq!(host.cell("A2"), Value::Long(8));
    p.run("Sheet1.Hello", &[], &mut host).unwrap();
}
