//! UserForms run without a window: a scripted host fills controls and clicks buttons.

mod common;

use std::collections::VecDeque;

use common::MockHost;
use kantor_script::forms::{Action, Control, ControlKind, Edit, FormDesign, FormInput, FormView};
use kantor_script::vba::{ModuleKind, Project};
use kantor_script::{Host, ObjectRef, ScriptError, Value};

type Step = Box<dyn FnMut(&FormView, &MockHost) -> FormInput>;

/// The mock spreadsheet plus scripted form steps.
struct FormHost {
    inner: MockHost,
    steps: VecDeque<Step>,
    views: Vec<FormView>,
    closed: Vec<String>,
}

impl FormHost {
    fn new(steps: Vec<Step>) -> FormHost {
        FormHost {
            inner: MockHost::new(),
            steps: steps.into(),
            views: Vec::new(),
            closed: Vec::new(),
        }
    }
}

impl Host for FormHost {
    fn globals(&mut self) -> Option<ObjectRef> {
        self.inner.globals()
    }

    fn constant(&mut self, name: &str) -> Option<Value> {
        self.inner.constant(name)
    }

    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> Result<i32, ScriptError> {
        self.inner.msg_box(prompt, buttons, title)
    }

    fn form_event(&mut self, view: &FormView) -> Result<FormInput, ScriptError> {
        self.views.push(view.clone());
        let Some(mut step) = self.steps.pop_front() else {
            panic!("the form is still shown and no step is left: {view:?}");
        };
        Ok(step(view, &self.inner))
    }

    fn form_closed(&mut self, name: &str) {
        self.closed.push(name.to_owned());
    }
}

fn ctl(name: &str, kind: ControlKind, tab: i16) -> Control {
    let mut c = Control::new(name, kind);
    c.tab_index = tab;
    c
}

fn form(controls: Vec<Control>) -> FormDesign {
    let mut d = FormDesign::new("UserForm1");
    d.controls = controls;
    for (i, c) in d.controls.iter_mut().enumerate() {
        c.id = i as i32 + 1;
    }
    d
}

fn project(design: FormDesign, form_code: &str, module_code: &str) -> Project {
    let mut p = Project::new();
    p.add_module("Module1", ModuleKind::Standard, module_code)
        .unwrap();
    p.add_module("UserForm1", ModuleKind::Form, form_code)
        .unwrap();
    p.add_form(design);
    p
}

fn i(view: &FormView, name: &str) -> usize {
    view.control(name).unwrap()
}

#[test]
fn handlers_write_cells_after_the_user_fills_the_form() {
    let mut list = ctl("lstCity", ControlKind::ListBox, 2);
    list.row_source = "A10:A12".into();
    let mut ok = ctl("cmdOK", ControlKind::CommandButton, 3);
    ok.default = true;
    let design = form(vec![
        ctl("txtName", ControlKind::TextBox, 0),
        ctl("chkVip", ControlKind::CheckBox, 1),
        list,
        ok,
    ]);
    let form_code = r#"
Public Done As Boolean
Private Sub UserForm_Initialize()
    Me.Caption = "Customer"
    txtName.Value = "Anon"
End Sub
Private Sub cmdOK_Click()
    Range("B1").Value = txtName.Text
    Range("B2").Value = chkVip.Value
    Range("B3").Value = lstCity.Value
    Range("B4").Value = lstCity.ListIndex
    Done = True
    Me.Hide
End Sub
"#;
    let module = r#"
Sub Go()
    UserForm1.Show
    Range("C1").Value = UserForm1.Done
    Range("C2").Value = UserForm1.txtName.Text
    Unload UserForm1
End Sub
"#;
    let mut p = project(design, form_code, module);
    let mut host = FormHost::new(vec![Box::new(|v: &FormView, _: &MockHost| {
        assert_eq!(v.design.caption, "Customer");
        assert_eq!(v.state[i(v, "txtName")].text, "Anon");
        let list = &v.state[i(v, "lstCity")].list;
        assert_eq!(list.len(), 3);
        assert_eq!(list[1], ["Rome"]);
        assert_eq!(v.focus, Some(i(v, "txtName")));
        assert!(v.modal);
        FormInput {
            edits: vec![
                Edit::Text {
                    control: i(v, "txtName"),
                    text: "Ann".into(),
                },
                Edit::Checked {
                    control: i(v, "chkVip"),
                    checked: Some(true),
                },
                Edit::ListIndex {
                    control: i(v, "lstCity"),
                    index: 1,
                },
            ],
            action: Action::DefaultKey,
        }
    })]);
    for (cell, city) in [("A10", "Paris"), ("A11", "Rome"), ("A12", "Oslo")] {
        host.inner.set(cell, city);
    }
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("B1"), Value::from("Ann"));
    assert_eq!(h.cell("B2"), Value::Boolean(true));
    assert_eq!(h.cell("B3"), Value::from("Rome"));
    assert_eq!(h.cell("B4"), Value::Long(1));
    assert_eq!(h.cell("C1"), Value::Boolean(true));
    assert_eq!(h.cell("C2"), Value::from("Ann"));
    assert_eq!(host.views.len(), 1);
    assert_eq!(host.closed, ["UserForm1"]);
}

#[test]
fn query_close_can_cancel_the_close_box() {
    let design = form(vec![]);
    let form_code = r#"
Private Sub UserForm_QueryClose(Cancel As Integer, CloseMode As Integer)
    Range("A1").Value = Range("A1").Value + 1
    Range("A2").Value = CloseMode
    If Range("A1").Value < 2 Then Cancel = True
End Sub
Private Sub UserForm_Terminate()
    Range("A3").Value = "bye"
End Sub
"#;
    let mut p = project(
        design,
        form_code,
        "Sub Go()\n UserForm1.Show\n Range(\"B1\") = \"back\"\nEnd Sub\n",
    );
    let close =
        || -> Step { Box::new(|_: &FormView, _: &MockHost| FormInput::action(Action::Close)) };
    let mut host = FormHost::new(vec![close(), close()]);
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("A1").as_f64(), Some(2.0));
    assert_eq!(h.cell("A2").as_f64(), Some(0.0));
    assert_eq!(h.cell("A3"), Value::from("bye"));
    assert_eq!(h.cell("B1"), Value::from("back"));
    assert_eq!(host.views.len(), 2);
}

#[test]
fn unload_me_in_a_click_handler_and_showing_again_loads_a_fresh_form() {
    let design = form(vec![
        ctl("txtNote", ControlKind::TextBox, 0),
        ctl("cmdClose", ControlKind::CommandButton, 1),
    ]);
    let form_code = r#"
Private Sub UserForm_Initialize()
    Range("C1") = Range("C1") + 1
End Sub
Private Sub cmdClose_Click()
    Range("A1") = "clicked " & txtNote.Text
    Unload Me
    Range("A2") = "after unload"
End Sub
Private Sub UserForm_QueryClose(Cancel As Integer, CloseMode As Integer)
    Range("A3") = CloseMode
End Sub
"#;
    let module = "Sub Go()\n UserForm1.Show\n UserForm1.Show\n Range(\"B1\") = \"back\"\nEnd Sub\n";
    let mut p = project(design, form_code, module);
    let first: Step = Box::new(|v: &FormView, _: &MockHost| FormInput {
        edits: vec![Edit::Text {
            control: i(v, "txtNote"),
            text: "one".into(),
        }],
        action: Action::Click(i(v, "cmdClose")),
    });
    let second: Step = Box::new(|v: &FormView, _: &MockHost| {
        // A fresh instance: the text typed before is gone.
        assert_eq!(v.state[i(v, "txtNote")].text, "");
        FormInput::action(Action::Click(i(v, "cmdClose")))
    });
    let mut host = FormHost::new(vec![first, second]);
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("A1"), Value::from("clicked "));
    assert_eq!(h.cell("A2"), Value::from("after unload"));
    assert_eq!(h.cell("A3").as_f64(), Some(1.0));
    assert_eq!(h.cell("B1"), Value::from("back"));
    assert_eq!(h.cell("C1").as_f64(), Some(2.0));
    assert_eq!(host.closed, ["UserForm1", "UserForm1"]);
}

#[test]
fn change_update_exit_and_enter_events() {
    let design = form(vec![
        ctl("txtAge", ControlKind::TextBox, 0),
        ctl("txtName", ControlKind::TextBox, 1),
        ctl("cmdOK", ControlKind::CommandButton, 2),
    ]);
    let form_code = r#"
Private Sub txtAge_Change()
    Range("A1") = Range("A1") + 1
End Sub
Private Sub txtAge_BeforeUpdate(ByVal Cancel As MSForms.ReturnBoolean)
    If Not IsNumeric(txtAge.Text) Then Cancel = True: Range("A2") = "bad"
End Sub
Private Sub txtAge_AfterUpdate()
    Range("A3") = txtAge.Text
End Sub
Private Sub txtName_Enter()
    Range("A4") = "entered"
End Sub
Private Sub txtName_Exit(ByVal Cancel As MSForms.ReturnBoolean)
    Range("A5") = "left"
End Sub
Private Sub cmdOK_Click()
    Unload Me
End Sub
"#;
    let mut p = project(design, form_code, "Sub Go()\n UserForm1.Show\nEnd Sub\n");
    let steps: Vec<Step> = vec![
        Box::new(|v: &FormView, _: &MockHost| {
            assert_eq!(v.focus, Some(0));
            assert!(v.live.contains(&0));
            FormInput {
                edits: vec![Edit::Text {
                    control: 0,
                    text: "x".into(),
                }],
                action: Action::Focus(1),
            }
        }),
        Box::new(|v: &FormView, h: &MockHost| {
            // BeforeUpdate cancelled: the focus stayed.
            assert_eq!(v.focus, Some(0));
            assert_eq!(h.cell("A2"), Value::from("bad"));
            FormInput {
                edits: vec![Edit::Text {
                    control: 0,
                    text: "42".into(),
                }],
                action: Action::Focus(1),
            }
        }),
        Box::new(|v: &FormView, h: &MockHost| {
            assert_eq!(v.focus, Some(1));
            assert_eq!(h.cell("A3"), Value::from("42"));
            assert_eq!(h.cell("A4"), Value::from("entered"));
            FormInput::action(Action::Click(2))
        }),
    ];
    let mut host = FormHost::new(steps);
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("A1").as_f64(), Some(2.0));
    assert_eq!(h.cell("A5"), Value::from("left"));
}

#[test]
fn lists_controls_collection_and_events_from_code() {
    let mut list = ctl("lstItems", ControlKind::ListBox, 0);
    list.multi_select = 1;
    list.column_count = 2;
    let design = form(vec![
        list,
        ctl("cboKind", ControlKind::ComboBox, 1),
        ctl("cmdOK", ControlKind::CommandButton, 2),
    ]);
    let form_code = r#"
Private Sub UserForm_Initialize()
    Dim i As Long
    For i = 1 To 3
        lstItems.AddItem "Item " & i
        lstItems.List(i - 1, 1) = i * 10
    Next
    cboKind.List = Array("A", "B", "C")
    cboKind.ListIndex = 2
End Sub
Private Sub cboKind_Change()
    Range("A1") = cboKind.Value
End Sub
Private Sub cmdOK_Click()
    Dim c As Control, n As Long, s As String, i As Long
    For Each c In Me.Controls
        If TypeName(c) = "ListBox" Then n = n + 1
    Next
    Range("A2") = n
    For i = 0 To lstItems.ListCount - 1
        If lstItems.Selected(i) Then s = s & lstItems.List(i, 1) & ";"
    Next
    Range("A3") = s
    Range("A4") = Me.Controls.Count
    Range("A5") = Me.Controls("cboKind").Text
    lstItems.RemoveItem 0
    Range("A6") = lstItems.ListCount & " " & lstItems.List(0)
    Hide
End Sub
"#;
    let mut p = project(design, form_code, "Sub Go()\n UserForm1.Show\nEnd Sub\n");
    let steps: Vec<Step> = vec![Box::new(|v: &FormView, h: &MockHost| {
        let lst = &v.state[0];
        assert_eq!(lst.list[0], ["Item 1", "10"]);
        assert_eq!(v.state[1].list_index, 2);
        assert_eq!(v.state[1].text, "C");
        assert_eq!(h.cell("A1"), Value::from("C"));
        FormInput {
            edits: vec![
                Edit::Selected {
                    control: 0,
                    row: 0,
                    selected: true,
                },
                Edit::Selected {
                    control: 0,
                    row: 2,
                    selected: true,
                },
            ],
            action: Action::Click(2),
        }
    })];
    let mut host = FormHost::new(steps);
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("A2"), Value::Long(1));
    assert_eq!(h.cell("A3"), Value::from("10;30;"));
    assert_eq!(h.cell("A4"), Value::Long(3));
    assert_eq!(h.cell("A5"), Value::from("C"));
    assert_eq!(h.cell("A6"), Value::from("2 Item 2"));
}

#[test]
fn new_instances_are_separate_from_the_default_one() {
    let design = form(vec![
        ctl("txtName", ControlKind::TextBox, 0),
        ctl("cmdHide", ControlKind::CommandButton, 1),
    ]);
    let form_code = r#"
Private Sub UserForm_Initialize()
    Range("B1") = Range("B1") + 1
End Sub
Private Sub cmdHide_Click()
    Me.Hide
End Sub
"#;
    let module = r#"
Sub Go()
    Dim f As New UserForm1
    f.Tag = "mine"
    Load UserForm1
    Range("A1") = (f Is UserForm1)
    f.Show
    Range("A2") = f.txtName.Text & " " & f.Tag & " " & TypeName(f.txtName)
    Range("A3") = UserForm1.txtName.Text
End Sub
"#;
    let mut p = project(design, form_code, module);
    let steps: Vec<Step> = vec![Box::new(|_: &FormView, _: &MockHost| FormInput {
        edits: vec![Edit::Text {
            control: 0,
            text: "zz".into(),
        }],
        action: Action::Click(1),
    })];
    let mut host = FormHost::new(steps);
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("A1"), Value::Boolean(false));
    assert_eq!(h.cell("A2"), Value::from("zz mine TextBox"));
    assert_eq!(h.cell("A3"), Value::from(""));
    assert_eq!(h.cell("B1").as_f64(), Some(2.0));
}

#[test]
fn option_buttons_in_a_group_and_key_press() {
    let mut a = ctl("optA", ControlKind::OptionButton, 0);
    a.value = "1".into();
    let design = form(vec![
        a,
        ctl("optB", ControlKind::OptionButton, 1),
        ctl("txtCode", ControlKind::TextBox, 2),
        ctl("cmdOK", ControlKind::CommandButton, 3),
    ]);
    let form_code = r#"
Private Sub optB_Click()
    Range("A1") = "B clicked"
End Sub
Private Sub txtCode_KeyPress(ByVal KeyAscii As MSForms.ReturnInteger)
    If KeyAscii < 48 Or KeyAscii > 57 Then KeyAscii = 0
End Sub
Private Sub cmdOK_Click()
    Range("A2") = optA.Value
    Range("A3") = optB.Value
    Range("A4") = txtCode.Text
    Unload Me
End Sub
"#;
    let mut p = project(design, form_code, "Sub Go()\n UserForm1.Show\nEnd Sub\n");
    let key = |ch: char| -> Step {
        Box::new(move |v: &FormView, _: &MockHost| {
            assert_eq!(v.key_press, [2]);
            FormInput::action(Action::KeyPress { control: 2, ch })
        })
    };
    let steps: Vec<Step> = vec![
        Box::new(|_: &FormView, _: &MockHost| FormInput {
            edits: vec![Edit::Checked {
                control: 1,
                checked: Some(true),
            }],
            action: Action::None,
        }),
        key('4'),
        key('x'),
        key('2'),
        Box::new(|v: &FormView, _: &MockHost| {
            assert_eq!(v.state[0].checked, Some(false));
            FormInput::action(Action::Click(3))
        }),
    ];
    let mut host = FormHost::new(steps);
    p.run("Go", &[], &mut host).unwrap();
    let h = &host.inner;
    assert_eq!(h.cell("A1"), Value::from("B clicked"));
    assert_eq!(h.cell("A2"), Value::Boolean(false));
    assert_eq!(h.cell("A3"), Value::Boolean(true));
    assert_eq!(h.cell("A4"), Value::from("42"));
}

#[test]
fn modeless_forms_run_after_the_macro() {
    let design = form(vec![ctl("cmdClose", ControlKind::CommandButton, 0)]);
    let form_code =
        "Private Sub cmdClose_Click()\n Range(\"A2\") = \"closed\"\n Unload Me\nEnd Sub\n";
    let module = "Sub Go()\n UserForm1.Show vbModeless\n Range(\"A1\") = \"after show\"\nEnd Sub\n";
    let mut p = project(design, form_code, module);
    let steps: Vec<Step> = vec![Box::new(|v: &FormView, h: &MockHost| {
        assert!(!v.modal);
        assert_eq!(h.cell("A1"), Value::from("after show"));
        FormInput::action(Action::Click(0))
    })];
    let mut host = FormHost::new(steps);
    p.run("Go", &[], &mut host).unwrap();
    assert_eq!(host.inner.cell("A2"), Value::from("closed"));
}

#[test]
fn showing_a_form_without_a_window_is_a_clear_error() {
    let mut p = project(form(vec![]), "", "Sub Go()\n UserForm1.Show\nEnd Sub\n");
    let mut host = MockHost::new();
    let e = p.run("Go", &[], &mut host).unwrap_err();
    assert_eq!(e.number, 445);
    assert!(e.description.contains("UserForm1"), "{}", e.description);
    // A form's procedures aren't macros.
    let mut p = project(form(vec![]), "Public Sub Hello()\nEnd Sub\n", "");
    assert!(p.run("UserForm1.Hello", &[], &mut host).is_err());
}

#[test]
fn errors_in_handlers_end_the_run_and_close_the_form() {
    let design = form(vec![ctl("cmdBad", ControlKind::CommandButton, 0)]);
    let form_code = "Private Sub cmdBad_Click()\n Dim x\n x = 1 / 0\nEnd Sub\n";
    let mut p = project(design, form_code, "Sub Go()\n UserForm1.Show\nEnd Sub\n");
    let steps: Vec<Step> = vec![Box::new(|_: &FormView, _: &MockHost| {
        FormInput::action(Action::Click(0))
    })];
    let mut host = FormHost::new(steps);
    let e = p.run("Go", &[], &mut host).unwrap_err();
    assert_eq!(e.number, 11);
    assert_eq!(host.closed, ["UserForm1"]);
}
