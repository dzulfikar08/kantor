//! UserForms in the VBA runtime.
//!
//! A form module's instance (an [`Instance`] with a [`FormRt`]) holds the form's state: its
//! design as the code changed it, each control's value and list, the focus, whether it's loaded
//! and shown. Controls are [`ControlObj`]s (`TextBox1.Value`, `ListBox1.AddItem`), `Controls` a
//! [`ControlsObj`]. Changing a value queues the events it causes (`Change`, `Click`); the
//! interpreter runs them right after the statement that changed it ([`Interp::form_drain`]), as
//! VBA does.
//!
//! `Show` runs the form loop ([`Interp::form_loop`]): the host gets a [`FormView`] and returns
//! the user's next step ([`FormInput`]), whose handlers run here, until the form is hidden or
//! unloaded. A modeless form (`Show vbModeless`) runs its loop once the macro has finished.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use super::ast::Ty;
use super::compile::Module;
use super::convert;
use super::interp::{Access, ArgIn, Interp, Place, R};
use super::objects::{Instance, Scope, new_slot};
use crate::forms::{
    Action, ControlKind, ControlState, Edit, Font, FormDesign, FormInput, FormView,
};
use crate::object::*;

/// What a state change still has to run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pending {
    /// An event without arguments (`Change`, `Click`) of a control, or of the form (`None`).
    Event(Option<usize>, &'static str),
    /// Write a control's value to its ControlSource cell.
    Store(usize),
    /// Move the focus (`SetFocus`): runs Exit / Enter.
    Focus(usize),
}

/// A form instance's state.
pub(crate) struct FormState {
    pub design: FormDesign,
    pub state: Vec<ControlState>,
    pub loaded: bool,
    pub visible: bool,
    pub modal: bool,
    pub focus: Option<usize>,
    /// Changed since it got the focus (for BeforeUpdate / AfterUpdate).
    pub dirty: Vec<bool>,
    pub pending: VecDeque<Pending>,
    pub position: Option<(f32, f32)>,
    pub tag: String,
}

fn initial_state(c: &crate::forms::Control) -> ControlState {
    let mut s = ControlState {
        list_index: -1,
        ..ControlState::default()
    };
    match c.kind {
        ControlKind::TextBox | ControlKind::ComboBox => s.text = c.value.clone(),
        ControlKind::CheckBox | ControlKind::OptionButton | ControlKind::ToggleButton => {
            s.checked = match c.value.as_str() {
                "1" => Some(true),
                "0" | "" => Some(false),
                _ => None,
            };
        }
        ControlKind::ScrollBar | ControlKind::SpinButton => s.number = c.position,
        ControlKind::MultiPage | ControlKind::TabStrip => s.number = c.tab_index_selected.max(0),
        _ => {}
    }
    s
}

impl FormState {
    fn new(design: &FormDesign) -> FormState {
        FormState {
            state: design.controls.iter().map(initial_state).collect(),
            dirty: vec![false; design.controls.len()],
            design: design.clone(),
            loaded: true,
            visible: false,
            modal: true,
            focus: None,
            pending: VecDeque::new(),
            position: None,
            tag: String::new(),
        }
    }
}

/// A form instance's run-time part.
pub(crate) struct FormRt {
    pub module: Rc<Module>,
    original: FormDesign,
    pub st: RefCell<FormState>,
    me: RefCell<Option<Weak<dyn Dispatch>>>,
}

fn is(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn arg(args: &[Arg], i: usize, name: &str) -> Option<Value> {
    find_arg(args, i, name).filter(|v| !v.is_missing()).cloned()
}

fn int(v: &Value) -> R<i32> {
    Ok(convert::to_i64(v)? as i32)
}

fn points(v: &Value) -> R<f32> {
    Ok(convert::to_f64(v)? as f32)
}

fn has_value(kind: ControlKind) -> bool {
    !matches!(
        kind,
        ControlKind::Label
            | ControlKind::CommandButton
            | ControlKind::Frame
            | ControlKind::Image
            | ControlKind::Page
            | ControlKind::Other(_)
    )
}

/// Whether a control can take the focus.
pub(crate) fn focusable(kind: ControlKind) -> bool {
    !matches!(
        kind,
        ControlKind::Label
            | ControlKind::Frame
            | ControlKind::Image
            | ControlKind::Page
            | ControlKind::MultiPage
            | ControlKind::Other(_)
    )
}

impl FormRt {
    pub fn new(module: Rc<Module>, design: FormDesign) -> Rc<FormRt> {
        Rc::new(FormRt {
            module,
            st: RefCell::new(FormState::new(&design)),
            original: design,
            me: RefCell::new(None),
        })
    }

    /// Back to the design (`Unload` then a new `Load`).
    pub fn reset(&self) {
        *self.st.borrow_mut() = FormState::new(&self.original);
    }

    pub fn set_me(&self, o: &ObjectRef) {
        *self.me.borrow_mut() = Some(Rc::downgrade(o));
    }

    pub fn me(&self) -> Option<ObjectRef> {
        self.me.borrow().as_ref()?.upgrade()
    }

    pub fn name(&self) -> String {
        self.st.borrow().design.name.clone()
    }

    pub fn control_index(&self, name: &str) -> Option<usize> {
        self.st
            .borrow()
            .design
            .controls
            .iter()
            .position(|c| is(&c.name, name))
    }

    pub fn take_pending(&self) -> Option<Pending> {
        self.st.borrow_mut().pending.pop_front()
    }

    fn push(&self, p: Pending) {
        self.st.borrow_mut().pending.push_back(p);
    }

    /// What the host draws. `live` / `key_press` come from the module's handlers.
    pub fn view(&self) -> FormView {
        let st = self.st.borrow();
        let handler = |name: &str, events: &[&str]| {
            events.iter().any(|e| {
                self.module
                    .procs
                    .contains_key(format!("{name}_{e}").to_lowercase().as_str())
            })
        };
        let mut live = Vec::new();
        let mut key_press = Vec::new();
        for (i, c) in st.design.controls.iter().enumerate() {
            if handler(&c.name, &["Change", "KeyPress", "KeyDown", "KeyUp"]) {
                live.push(i);
            }
            if handler(&c.name, &["KeyPress"]) {
                key_press.push(i);
            }
        }
        FormView {
            design: st.design.clone(),
            state: st.state.clone(),
            focus: st.focus,
            live,
            key_press,
            modal: st.modal,
            position: st.position,
        }
    }

    /// Whether `key` (lowercase) names a member of the form object: a control, or a property
    /// or method of a UserForm.
    pub fn has_member(&self, key: &str) -> bool {
        FORM_MEMBERS.contains(&key) || self.control_index(key).is_some()
    }

    pub fn control(self: &Rc<Self>, i: usize) -> ObjectRef {
        Rc::new(ControlObj {
            rt: self.clone(),
            i,
        })
    }

    /// The controls inside `parent` (all of them, nested ones too, for the form).
    pub fn controls(self: &Rc<Self>, parent: Option<usize>, type_name: &'static str) -> ObjectRef {
        let st = self.st.borrow();
        let items: Vec<usize> = match parent {
            None => (0..st.design.controls.len())
                .filter(|&i| st.design.controls[i].kind != ControlKind::Page)
                .collect(),
            Some(p) => {
                // Descendants, as Frame.Controls lists nested controls too.
                let mut out = Vec::new();
                for i in 0..st.design.controls.len() {
                    let mut at = st.design.controls[i].parent;
                    while let Some(a) = at {
                        if a == p {
                            if type_name == "Pages" {
                                if st.design.controls[i].parent == Some(p) {
                                    out.push(i);
                                }
                            } else if st.design.controls[i].kind != ControlKind::Page {
                                out.push(i);
                            }
                            break;
                        }
                        at = st.design.controls[a].parent;
                    }
                }
                out
            }
        };
        Rc::new(ControlsObj {
            rt: self.clone(),
            items,
            type_name,
        })
    }

    // ---- The form's own properties ----

    pub fn form_get(self: &Rc<Self>, key: &str, args: &[Arg]) -> R<Value> {
        if let Some(i) = self.control_index(key) {
            let c = self.control(i);
            return if args.is_empty() {
                Ok(Value::object(c))
            } else {
                c.get("", args)
            };
        }
        match key {
            "" | "controls" => {
                let c = self.controls(None, "Controls");
                return if args.is_empty() {
                    Ok(Value::object(c))
                } else {
                    c.get("Item", args)
                };
            }
            "activecontrol" => {
                let focus = self.st.borrow().focus;
                return Ok(match focus {
                    Some(i) => Value::object(self.control(i)),
                    None => Value::NOTHING,
                });
            }
            "font" => {
                return Ok(Value::object(Rc::new(FontObj {
                    rt: self.clone(),
                    target: None,
                })));
            }
            _ => {}
        }
        let st = self.st.borrow();
        let d = &st.design;
        Ok(match key {
            "caption" => Value::String(d.caption.clone()),
            "name" => Value::String(d.name.clone()),
            "tag" => Value::String(st.tag.clone()),
            "width" => Value::Single(d.width + 4.0),
            "height" => Value::Single(d.height + 24.0),
            "insidewidth" => Value::Single(d.width),
            "insideheight" => Value::Single(d.height),
            "left" => Value::Single(st.position.map_or(0.0, |p| p.0)),
            "top" => Value::Single(st.position.map_or(0.0, |p| p.1)),
            "backcolor" => Value::Long(d.back_color as i32),
            "forecolor" => Value::Long(d.fore_color as i32),
            "bordercolor" => Value::Long(d.border_color as i32),
            "enabled" => Value::Boolean(d.enabled),
            "visible" => Value::Boolean(st.visible),
            "startupposition" => Value::Long(d.start_up_position),
            "picture" | "mouseicon" => Value::NOTHING,
            "zoom" => Value::Long(100),
            "scrolltop" | "scrollleft" => Value::Long(0),
            "repaint" | "printform" | "zorder" | "move" | "setfocus" | "redoaction"
            | "undoaction" | "setdefaulttaborder" => Value::Empty,
            _ => return Err(ScriptError::no_member(&d.name, key)),
        })
    }

    pub fn form_put(&self, key: &str, _args: &[Arg], value: Value) -> R<()> {
        let mut st = self.st.borrow_mut();
        match key {
            "caption" => st.design.caption = convert::to_str(&value)?,
            "tag" => st.tag = convert::to_str(&value)?,
            "width" => st.design.width = (points(&value)? - 4.0).max(0.0),
            "height" => st.design.height = (points(&value)? - 24.0).max(0.0),
            "insidewidth" => st.design.width = points(&value)?.max(0.0),
            "insideheight" => st.design.height = points(&value)?.max(0.0),
            "left" => {
                let top = st.position.map_or(0.0, |p| p.1);
                st.position = Some((points(&value)?, top));
            }
            "top" => {
                let left = st.position.map_or(0.0, |p| p.0);
                st.position = Some((left, points(&value)?));
            }
            "backcolor" => st.design.back_color = int(&value)? as u32,
            "forecolor" => st.design.fore_color = int(&value)? as u32,
            "bordercolor" => st.design.border_color = int(&value)? as u32,
            "enabled" => st.design.enabled = convert::to_bool(&value)?,
            "startupposition" => st.design.start_up_position = int(&value)?,
            "scrolltop" | "scrollleft" | "zoom" | "picture" | "mouseicon" => {}
            _ => return Err(ScriptError::no_member(&st.design.name, key)),
        }
        Ok(())
    }

    // ---- Values ----

    /// Sets a control's state and queues the events the change causes.
    fn set_text(&self, i: usize, text: String) {
        let mut st = self.st.borrow_mut();
        if st.state[i].text == text {
            return;
        }
        let kind = st.design.controls[i].kind;
        st.state[i].text = text.clone();
        if kind == ControlKind::ComboBox {
            let col = text_col(&st.design.controls[i]);
            let found = st.state[i]
                .list
                .iter()
                .position(|r| r.get(col).is_some_and(|c| *c == text))
                .map_or(-1, |n| n as i32);
            st.state[i].list_index = found;
        }
        drop(st);
        self.changed(i, false);
    }

    fn set_checked(&self, i: usize, checked: Option<bool>) {
        let mut st = self.st.borrow_mut();
        if st.state[i].checked == checked {
            return;
        }
        st.state[i].checked = checked;
        let c = st.design.controls[i].clone();
        let mut others = Vec::new();
        if c.kind == ControlKind::OptionButton && checked == Some(true) {
            for (j, o) in st.design.controls.iter().enumerate() {
                let same_group = if c.group_name.is_empty() {
                    o.group_name.is_empty() && o.parent == c.parent
                } else {
                    is(&o.group_name, &c.group_name)
                };
                if j != i && o.kind == ControlKind::OptionButton && same_group {
                    others.push(j);
                }
            }
            // Those already off get no event.
            others.retain(|&j| st.state[j].checked != Some(false));
            for &j in &others {
                st.state[j].checked = Some(false);
            }
        }
        drop(st);
        self.changed(i, true);
        for j in others {
            self.push(Pending::Event(Some(j), "Change"));
        }
    }

    fn set_list_index(&self, i: usize, index: i32) {
        let mut st = self.st.borrow_mut();
        let n = st.state[i].list.len() as i32;
        let index = if (0..n).contains(&index) { index } else { -1 };
        if st.state[i].list_index == index {
            return;
        }
        st.state[i].list_index = index;
        let c = st.design.controls[i].clone();
        if c.kind == ControlKind::ComboBox && index >= 0 {
            let col = text_col(&c);
            st.state[i].text = st.state[i].list[index as usize]
                .get(col)
                .cloned()
                .unwrap_or_default();
        }
        if c.kind == ControlKind::ListBox && c.multi_select == 0 {
            let len = st.state[i].list.len();
            st.state[i].selected = vec![false; len];
            if index >= 0 {
                st.state[i].selected[index as usize] = true;
            }
        }
        drop(st);
        self.changed(i, true);
    }

    fn set_selected(&self, i: usize, row: usize, on: bool) -> R<()> {
        let mut st = self.st.borrow_mut();
        let len = st.state[i].list.len();
        if row >= len {
            return Err(ScriptError::new(
                381,
                "Could not set the Selected property. Invalid property array index.",
            ));
        }
        if st.state[i].selected.len() != len {
            st.state[i].selected.resize(len, false);
        }
        if st.state[i].selected[row] == on {
            return Ok(());
        }
        let multi = st.design.controls[i].multi_select != 0;
        if !multi {
            drop(st);
            self.set_list_index(i, if on { row as i32 } else { -1 });
            return Ok(());
        }
        st.state[i].selected[row] = on;
        st.state[i].list_index = row as i32;
        drop(st);
        self.changed(i, false);
        Ok(())
    }

    fn set_number(&self, i: usize, value: i32) {
        let mut st = self.st.borrow_mut();
        let c = &st.design.controls[i];
        let value = match c.kind {
            ControlKind::ScrollBar | ControlKind::SpinButton => {
                let (lo, hi) = (c.min.min(c.max), c.min.max(c.max));
                value.clamp(lo, hi)
            }
            ControlKind::MultiPage => {
                let pages = st.design.children(Some(i)).len() as i32;
                value.clamp(0, (pages - 1).max(0))
            }
            ControlKind::TabStrip => value.clamp(-1, c.tabs.len() as i32 - 1),
            _ => value,
        };
        if st.state[i].number == value {
            return;
        }
        st.state[i].number = value;
        drop(st);
        self.changed(i, false);
    }

    /// Queues Change (and Click for buttons and lists), marks the control changed and its
    /// ControlSource to be written.
    fn changed(&self, i: usize, click: bool) {
        let mut st = self.st.borrow_mut();
        st.dirty[i] = true;
        st.pending.push_back(Pending::Event(Some(i), "Change"));
        if click {
            st.pending.push_back(Pending::Event(Some(i), "Click"));
        }
        if !st.design.controls[i].control_source.is_empty() {
            st.pending.push_back(Pending::Store(i));
        }
    }

    /// Applies what the user changed.
    pub fn apply_edit(&self, e: &Edit) -> R<()> {
        let n = self.st.borrow().state.len();
        if e.control() >= n {
            return Err(ScriptError::runtime(9));
        }
        match e {
            Edit::Text { control, text } => self.set_text(*control, text.clone()),
            Edit::Checked { control, checked } => self.set_checked(*control, *checked),
            Edit::ListIndex { control, index } => self.set_list_index(*control, *index),
            Edit::Selected {
                control,
                row,
                selected,
            } => self.set_selected(*control, *row, *selected)?,
            Edit::Number { control, value } => self.set_number(*control, *value),
        }
        Ok(())
    }

    /// A control's Value, as VBA gives it.
    pub fn value(&self, i: usize) -> Value {
        let st = self.st.borrow();
        let c = &st.design.controls[i];
        let s = &st.state[i];
        match c.kind {
            ControlKind::TextBox => Value::String(s.text.clone()),
            ControlKind::ComboBox => match bound_value(c, s) {
                Some(v) => v,
                None => Value::String(s.text.clone()),
            },
            ControlKind::ListBox => {
                if c.multi_select != 0 {
                    return Value::Null;
                }
                bound_value(c, s).unwrap_or(Value::Null)
            }
            ControlKind::CheckBox | ControlKind::OptionButton | ControlKind::ToggleButton => {
                match s.checked {
                    Some(b) => Value::Boolean(b),
                    None => Value::Null,
                }
            }
            ControlKind::ScrollBar
            | ControlKind::SpinButton
            | ControlKind::MultiPage
            | ControlKind::TabStrip => Value::Long(s.number),
            ControlKind::CommandButton => Value::Boolean(false),
            _ => Value::Empty,
        }
    }

    /// Sets a control's Value from the script.
    pub fn set_value(&self, i: usize, v: &Value) -> R<()> {
        let kind = self.st.borrow().design.controls[i].kind;
        match kind {
            ControlKind::TextBox => self.set_text(i, text_of(v)?),
            ControlKind::ComboBox => {
                let st = self.st.borrow();
                let c = st.design.controls[i].clone();
                let rows = st.state[i].list.clone();
                drop(st);
                let wanted = text_of(v)?;
                let bound = (c.bound_column.max(1) - 1) as usize;
                if c.bound_column > 0
                    && let Some(row) = rows
                        .iter()
                        .position(|r| r.get(bound).is_some_and(|x| *x == wanted))
                {
                    self.set_list_index(i, row as i32);
                } else {
                    self.set_text(i, wanted);
                }
            }
            ControlKind::ListBox => {
                if v.is_null() {
                    self.set_list_index(i, -1);
                    return Ok(());
                }
                let st = self.st.borrow();
                let c = st.design.controls[i].clone();
                let rows = st.state[i].list.clone();
                drop(st);
                if c.multi_select != 0 {
                    return Err(ScriptError::new(
                        380,
                        "Could not set the Value property. Invalid property value.",
                    ));
                }
                let row = if c.bound_column == 0 {
                    int(v)?
                } else {
                    let wanted = text_of(v)?;
                    let col = (c.bound_column - 1) as usize;
                    rows.iter()
                        .position(|r| r.get(col).is_some_and(|x| *x == wanted))
                        .map(|n| n as i32)
                        .ok_or_else(|| {
                            ScriptError::new(
                                380,
                                "Could not set the Value property. Invalid property value.",
                            )
                        })?
                };
                self.set_list_index(i, row);
            }
            ControlKind::CheckBox | ControlKind::OptionButton | ControlKind::ToggleButton => {
                let checked = if v.is_null() {
                    None
                } else {
                    Some(convert::to_bool(v)?)
                };
                self.set_checked(i, checked);
            }
            ControlKind::ScrollBar
            | ControlKind::SpinButton
            | ControlKind::MultiPage
            | ControlKind::TabStrip => self.set_number(i, int(v)?),
            ControlKind::CommandButton => {}
            _ => return Err(ScriptError::no_member(kind.type_name(), "Value")),
        }
        Ok(())
    }

    /// Fills a list (RowSource, `List = array`) without events.
    pub fn set_rows(&self, i: usize, rows: Vec<Vec<String>>) {
        let mut st = self.st.borrow_mut();
        let c = st.design.controls[i].clone();
        let s = &mut st.state[i];
        s.selected = vec![false; rows.len()];
        s.list = rows;
        s.list_index = -1;
        if c.kind == ControlKind::ComboBox {
            let col = text_col(&c);
            s.list_index = s
                .list
                .iter()
                .position(|r| r.get(col).is_some_and(|x| *x == s.text))
                .map_or(-1, |n| n as i32);
        }
    }

    /// Sets a value read from a ControlSource cell, without events.
    pub fn load_value(&self, i: usize, v: &Value) {
        let kind = self.st.borrow().design.controls[i].kind;
        let mut st = self.st.borrow_mut();
        let s = &mut st.state[i];
        match kind {
            ControlKind::TextBox | ControlKind::ComboBox => s.text = convert::display(v),
            ControlKind::CheckBox | ControlKind::OptionButton | ControlKind::ToggleButton => {
                s.checked = convert::to_bool(v).ok();
            }
            ControlKind::ScrollBar | ControlKind::SpinButton => {
                s.number = convert::to_i64(v).map_or(s.number, |n| n as i32);
            }
            ControlKind::ListBox => {
                let text = convert::display(v);
                s.list_index = s
                    .list
                    .iter()
                    .position(|r| r.first().is_some_and(|x| *x == text))
                    .map_or(-1, |n| n as i32);
            }
            _ => {}
        }
    }

    pub fn focus_first(&self) {
        let mut st = self.st.borrow_mut();
        if st.focus.is_some() {
            return;
        }
        let view_state = st.state.clone();
        let view = FormView {
            design: st.design.clone(),
            state: view_state,
            focus: None,
            live: Vec::new(),
            key_press: Vec::new(),
            modal: true,
            position: None,
        };
        let mut order: Vec<usize> = (0..st.design.controls.len())
            .filter(|&i| {
                let c = &st.design.controls[i];
                focusable(c.kind) && c.tab_stop && c.enabled && view.shown(i)
            })
            .collect();
        order.sort_by_key(|&i| {
            (
                st.design.controls[i].parent.is_some(),
                st.design.controls[i].tab_index,
            )
        });
        st.focus = order.first().copied();
    }

    // ---- Control properties ----

    fn ctl_get(self: &Rc<Self>, i: usize, name: &str, args: &[Arg]) -> R<Value> {
        let key = name.to_ascii_lowercase();
        let st = self.st.borrow();
        let c = st.design.controls[i].clone();
        let s = st.state[i].clone();
        drop(st);
        let list_cell = |row: i32, col: i32| -> R<Value> {
            let r = s
                .list
                .get(usize::try_from(row).map_err(|_| ScriptError::runtime(381))?)
                .ok_or_else(|| {
                    ScriptError::new(
                        381,
                        "Could not get the List property. Invalid property array index.",
                    )
                })?;
            Ok(match r.get(col.max(0) as usize) {
                Some(t) => Value::String(t.clone()),
                None => Value::Null,
            })
        };
        Ok(match key.as_str() {
            "" => {
                if !args.is_empty() && c.kind.is_container() {
                    return self.controls(Some(i), "Controls").get("Item", args);
                }
                match c.kind {
                    ControlKind::Label | ControlKind::Frame | ControlKind::Page => {
                        Value::String(c.caption)
                    }
                    _ => self.value(i),
                }
            }
            "value" => self.value(i),
            "text" => match c.kind {
                ControlKind::ListBox => {
                    if s.list_index >= 0 {
                        let col = text_col(&c);
                        Value::String(
                            s.list[s.list_index as usize]
                                .get(col)
                                .cloned()
                                .unwrap_or_default(),
                        )
                    } else {
                        Value::String(String::new())
                    }
                }
                ControlKind::TextBox | ControlKind::ComboBox => Value::String(s.text),
                _ => return Err(ScriptError::no_member(c.kind.type_name(), name)),
            },
            "caption" => Value::String(c.caption),
            "name" => Value::String(c.name),
            "tag" => Value::String(c.tag),
            "controltiptext" => Value::String(c.tip),
            "enabled" => Value::Boolean(c.enabled),
            "visible" => Value::Boolean(c.visible),
            "locked" => Value::Boolean(c.locked),
            "left" => Value::Single(c.left),
            "top" => Value::Single(c.top),
            "width" => Value::Single(c.width),
            "height" => Value::Single(c.height),
            "tabindex" => Value::Integer(c.tab_index.max(0)),
            "tabstop" => Value::Boolean(c.tab_stop),
            "backcolor" => Value::Long(c.back_color as i32),
            "forecolor" => Value::Long(c.fore_color as i32),
            "bordercolor" => Value::Long(c.border_color as i32),
            "borderstyle" => Value::Long(i32::from(c.border_style)),
            "specialeffect" => Value::Long(i32::from(c.special_effect)),
            "backstyle" => Value::Long(i32::from(c.back_opaque)),
            "controlsource" => Value::String(c.control_source),
            "rowsource" => Value::String(c.row_source),
            "groupname" => Value::String(c.group_name),
            "maxlength" => Value::Long(c.max_length as i32),
            "multiline" => Value::Boolean(c.multi_line),
            "wordwrap" => Value::Boolean(c.word_wrap),
            "autosize" => Value::Boolean(c.auto_size),
            "passwordchar" => Value::String(c.password_char.map(String::from).unwrap_or_default()),
            "accelerator" => Value::String(c.accelerator.map(String::from).unwrap_or_default()),
            "textalign" => Value::Long(i32::from(c.text_align)),
            "default" => Value::Boolean(c.default),
            "cancel" => Value::Boolean(c.cancel),
            "columncount" => Value::Long(c.column_count),
            "boundcolumn" => Value::Long(c.bound_column),
            "textcolumn" => Value::Long(c.text_column),
            "columnheads" => Value::Boolean(c.column_heads),
            "multiselect" => Value::Long(i32::from(c.multi_select)),
            "liststyle" => Value::Long(i32::from(c.list_style)),
            "listrows" => Value::Long(c.list_rows),
            "style" => Value::Long(if c.drop_list { 2 } else { 0 }),
            "matchentry" => Value::Long(i32::from(c.match_entry)),
            "min" => Value::Long(c.min),
            "max" => Value::Long(c.max),
            "smallchange" => Value::Long(c.small_change),
            "largechange" => Value::Long(c.large_change),
            "orientation" => Value::Long(c.orientation),
            "picturesizemode" => Value::Long(i32::from(c.picture_size_mode)),
            "picture" | "mouseicon" => Value::NOTHING,
            "listcount" => Value::Long(s.list.len() as i32),
            "listindex" => Value::Long(s.list_index),
            "topindex" => Value::Long(0),
            "seltext" => Value::String(String::new()),
            "selstart" => Value::Long(s.text.chars().count() as i32),
            "sellength" | "linecount" => Value::Long(0),
            "textlength" => Value::Long(s.text.chars().count() as i32),
            "index" => {
                let pages = self.st.borrow().design.children(c.parent);
                Value::Long(pages.iter().position(|&p| p == i).unwrap_or(0) as i32)
            }
            "selected" => {
                let row = int(&arg(args, 0, "index").ok_or_else(|| ScriptError::runtime(449))?)?;
                let on = usize::try_from(row)
                    .ok()
                    .and_then(|r| s.selected.get(r).copied())
                    .ok_or_else(|| {
                        ScriptError::new(
                            381,
                            "Could not get the Selected property. Invalid property array index.",
                        )
                    })?;
                Value::Boolean(on)
            }
            "list" => {
                let row = arg(args, 0, "pvargIndex");
                let col = arg(args, 1, "pvargColumn");
                match (row, col) {
                    (None, _) => list_array(&s.list, c.column_count),
                    (Some(r), None) => list_cell(int(&r)?, 0)?,
                    (Some(r), Some(cl)) => list_cell(int(&r)?, int(&cl)?)?,
                }
            }
            "column" => {
                let col = arg(args, 0, "pvargColumn");
                let row = arg(args, 1, "pvargIndex");
                match (col, row) {
                    (None, _) => {
                        // Columns as rows (the transposed list).
                        let cols = c.column_count.max(1) as usize;
                        let t: Vec<Vec<Value>> = (0..cols)
                            .map(|j| {
                                s.list
                                    .iter()
                                    .map(|r| {
                                        r.get(j).map_or(Value::Null, |x| Value::String(x.clone()))
                                    })
                                    .collect()
                            })
                            .collect();
                        Value::array(Array::from_rows(0, t))
                    }
                    (Some(cl), None) => {
                        let j = int(&cl)?;
                        if s.list_index < 0 {
                            Value::Null
                        } else {
                            list_cell(s.list_index, j)?
                        }
                    }
                    (Some(cl), Some(r)) => list_cell(int(&r)?, int(&cl)?)?,
                }
            }
            "controls" if c.kind.is_container() => {
                let ctl = self.controls(Some(i), "Controls");
                if args.is_empty() {
                    Value::object(ctl)
                } else {
                    ctl.get("Item", args)?
                }
            }
            "pages" if c.kind == ControlKind::MultiPage => {
                let pages = self.controls(Some(i), "Pages");
                if args.is_empty() {
                    Value::object(pages)
                } else {
                    pages.get("Item", args)?
                }
            }
            "selecteditem" if c.kind == ControlKind::MultiPage => {
                let pages = self.st.borrow().design.children(Some(i));
                match pages.get(s.number.max(0) as usize) {
                    Some(&p) => Value::object(self.control(p)),
                    None => Value::NOTHING,
                }
            }
            "font" => Value::object(Rc::new(FontObj {
                rt: self.clone(),
                target: Some(i),
            })),
            "object" => Value::object(self.control(i)),
            "parent" => match (c.parent, self.me()) {
                (Some(p), _) => Value::object(self.control(p)),
                (None, Some(me)) => Value::object(me),
                (None, None) => Value::NOTHING,
            },
            // Methods used as functions return nothing.
            "setfocus" | "additem" | "removeitem" | "clear" | "move" | "zorder" | "dropdown"
            | "copy" | "cut" | "paste" | "repaint" => {
                self.ctl_call(i, name, args)?;
                Value::Empty
            }
            _ => return Err(ScriptError::no_member(c.kind.type_name(), name)),
        })
    }

    fn ctl_put(&self, i: usize, name: &str, args: &[Arg], value: Value) -> R<()> {
        let key = name.to_ascii_lowercase();
        let kind = self.st.borrow().design.controls[i].kind;
        match key.as_str() {
            "" | "value" => {
                if key.is_empty()
                    && matches!(
                        kind,
                        ControlKind::Label | ControlKind::Frame | ControlKind::Page
                    )
                {
                    let text = convert::to_str(&value)?;
                    self.st.borrow_mut().design.controls[i].caption = text;
                    return Ok(());
                }
                return self.set_value(i, &value);
            }
            "text" => {
                if kind == ControlKind::ListBox {
                    let wanted = convert::to_str(&value)?;
                    let st = self.st.borrow();
                    let col = text_col(&st.design.controls[i]);
                    let row = st.state[i]
                        .list
                        .iter()
                        .position(|r| r.get(col).is_some_and(|x| *x == wanted));
                    drop(st);
                    self.set_list_index(i, row.map_or(-1, |r| r as i32));
                    return Ok(());
                }
                self.set_text(i, text_of(&value)?);
                return Ok(());
            }
            "listindex" => {
                let n = int(&value)?;
                let len = self.st.borrow().state[i].list.len() as i32;
                if n < -1 || n >= len {
                    return Err(ScriptError::new(
                        380,
                        "Could not set the ListIndex property. Invalid property value.",
                    ));
                }
                self.set_list_index(i, n);
                return Ok(());
            }
            "selected" => {
                let row = int(&arg(args, 0, "index").ok_or_else(|| ScriptError::runtime(449))?)?;
                let row = usize::try_from(row).map_err(|_| ScriptError::runtime(381))?;
                return self.set_selected(i, row, convert::to_bool(&value)?);
            }
            "list" => {
                let row = arg(args, 0, "pvargIndex");
                let col = arg(args, 1, "pvargColumn");
                match row {
                    None => {
                        let rows = rows_of(&value)?;
                        self.set_rows(i, rows);
                    }
                    Some(r) => {
                        let r = usize::try_from(int(&r)?).map_err(|_| ScriptError::runtime(381))?;
                        let cl = match col {
                            Some(c) => {
                                usize::try_from(int(&c)?).map_err(|_| ScriptError::runtime(381))?
                            }
                            None => 0,
                        };
                        let text = convert::display(&value);
                        let mut st = self.st.borrow_mut();
                        let list = &mut st.state[i].list;
                        let row = list.get_mut(r).ok_or_else(|| {
                            ScriptError::new(
                                381,
                                "Could not set the List property. Invalid property array index.",
                            )
                        })?;
                        if row.len() <= cl {
                            row.resize(cl + 1, String::new());
                        }
                        row[cl] = text;
                    }
                }
                return Ok(());
            }
            "column" => {
                let col = arg(args, 0, "pvargColumn");
                let row = arg(args, 1, "pvargIndex");
                match (col, row) {
                    (Some(cl), Some(r)) => {
                        return self.ctl_put(i, "List", &[Arg::new(r), Arg::new(cl)], value);
                    }
                    _ => {
                        // Column = array: columns given as rows.
                        let cols = rows_of(&value)?;
                        let n = cols.iter().map(Vec::len).max().unwrap_or(0);
                        let rows = (0..n)
                            .map(|r| {
                                cols.iter()
                                    .map(|c| c.get(r).cloned().unwrap_or_default())
                                    .collect()
                            })
                            .collect();
                        self.set_rows(i, rows);
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
        let mut st = self.st.borrow_mut();
        let c = &mut st.design.controls[i];
        match key.as_str() {
            "caption" => c.caption = convert::to_str(&value)?,
            "tag" => c.tag = convert::to_str(&value)?,
            "controltiptext" => c.tip = convert::to_str(&value)?,
            "enabled" => c.enabled = convert::to_bool(&value)?,
            "visible" => c.visible = convert::to_bool(&value)?,
            "locked" => c.locked = convert::to_bool(&value)?,
            "left" => c.left = points(&value)?,
            "top" => c.top = points(&value)?,
            "width" => c.width = points(&value)?.max(0.0),
            "height" => c.height = points(&value)?.max(0.0),
            "tabindex" => c.tab_index = int(&value)? as i16,
            "tabstop" => c.tab_stop = convert::to_bool(&value)?,
            "backcolor" => c.back_color = int(&value)? as u32,
            "forecolor" => c.fore_color = int(&value)? as u32,
            "bordercolor" => c.border_color = int(&value)? as u32,
            "borderstyle" => c.border_style = int(&value)? as u8,
            "specialeffect" => c.special_effect = int(&value)? as u8,
            "backstyle" => c.back_opaque = int(&value)? != 0,
            "controlsource" => c.control_source = convert::to_str(&value)?,
            "groupname" => c.group_name = convert::to_str(&value)?,
            "maxlength" => c.max_length = int(&value)?.max(0) as u32,
            "multiline" => c.multi_line = convert::to_bool(&value)?,
            "wordwrap" => c.word_wrap = convert::to_bool(&value)?,
            "autosize" => c.auto_size = convert::to_bool(&value)?,
            "passwordchar" => c.password_char = convert::to_str(&value)?.chars().next(),
            "accelerator" => c.accelerator = convert::to_str(&value)?.chars().next(),
            "textalign" => c.text_align = int(&value)? as u8,
            "default" => c.default = convert::to_bool(&value)?,
            "cancel" => c.cancel = convert::to_bool(&value)?,
            "columncount" => c.column_count = int(&value)?,
            "boundcolumn" => c.bound_column = int(&value)?,
            "textcolumn" => c.text_column = int(&value)?,
            "columnheads" => c.column_heads = convert::to_bool(&value)?,
            "multiselect" => c.multi_select = int(&value)? as u8,
            "liststyle" => c.list_style = int(&value)? as u8,
            "listrows" => c.list_rows = int(&value)?,
            "style" => c.drop_list = int(&value)? == 2,
            "matchentry" => c.match_entry = int(&value)? as u8,
            "min" => c.min = int(&value)?,
            "max" => c.max = int(&value)?,
            "smallchange" => c.small_change = int(&value)?,
            "largechange" => c.large_change = int(&value)?,
            "orientation" => c.orientation = int(&value)?,
            "picturesizemode" => c.picture_size_mode = int(&value)? as u8,
            "rowsource" => {
                c.row_source = convert::to_str(&value)?;
                // Filled by the interpreter (it reads the cells).
                st.pending
                    .push_back(Pending::Event(Some(i), "\u{1}RowSource"));
            }
            "picture" | "mouseicon" | "topindex" | "selstart" | "sellength" | "seltext"
            | "columnwidths" | "listwidth" | "showdropbuttonwhen" | "dropbuttonstyle"
            | "integralheight" | "enterkeybehavior" | "tabkeybehavior" | "scrollbars"
            | "mousepointer" | "takefocusonclick" | "hideselection" | "autotab"
            | "autowordselect" | "enterfieldbehavior" | "selectionmargin" => {}
            _ => return Err(ScriptError::no_member(c.kind.type_name(), name)),
        }
        Ok(())
    }

    fn ctl_call(&self, i: usize, name: &str, args: &[Arg]) -> R<()> {
        let key = name.to_ascii_lowercase();
        match key.as_str() {
            "setfocus" => self.push(Pending::Focus(i)),
            "additem" => {
                let mut st = self.st.borrow_mut();
                if !st.design.controls[i].row_source.is_empty() {
                    return Err(ScriptError::new(70, "Permission denied"));
                }
                let item =
                    arg(args, 0, "pvargItem").map_or_else(String::new, |v| convert::display(&v));
                let list = &mut st.state[i].list;
                let at = match arg(args, 1, "pvargIndex") {
                    Some(v) => {
                        let n = int(&v)?;
                        usize::try_from(n)
                            .ok()
                            .filter(|&n| n <= list.len())
                            .ok_or_else(|| ScriptError::new(-2147024809, "Invalid argument."))?
                    }
                    None => list.len(),
                };
                list.insert(at, vec![item]);
                let s = &mut st.state[i];
                if s.selected.len() < s.list.len() {
                    s.selected.insert(at.min(s.selected.len()), false);
                }
                if s.list_index >= at as i32 {
                    s.list_index += 1;
                }
            }
            "removeitem" => {
                let n = int(&arg(args, 0, "pvargIndex").ok_or_else(|| ScriptError::runtime(449))?)?;
                let mut st = self.st.borrow_mut();
                if !st.design.controls[i].row_source.is_empty() {
                    return Err(ScriptError::new(70, "Permission denied"));
                }
                let s = &mut st.state[i];
                let at = usize::try_from(n)
                    .ok()
                    .filter(|&n| n < s.list.len())
                    .ok_or_else(|| ScriptError::new(-2147024809, "Invalid argument."))?;
                s.list.remove(at);
                if at < s.selected.len() {
                    s.selected.remove(at);
                }
                if s.list_index == at as i32 {
                    s.list_index = -1;
                } else if s.list_index > at as i32 {
                    s.list_index -= 1;
                }
            }
            "clear" => {
                let mut st = self.st.borrow_mut();
                if !st.design.controls[i].row_source.is_empty() {
                    return Err(ScriptError::new(70, "Permission denied"));
                }
                let kind = st.design.controls[i].kind;
                let s = &mut st.state[i];
                if kind == ControlKind::TextBox {
                    drop(st);
                    self.set_text(i, String::new());
                    return Ok(());
                }
                let had = s.list_index;
                s.list.clear();
                s.selected.clear();
                s.list_index = -1;
                drop(st);
                if had >= 0 {
                    self.changed(i, false);
                }
            }
            "move" => {
                let mut st = self.st.borrow_mut();
                let c = &mut st.design.controls[i];
                if let Some(v) = arg(args, 0, "Left") {
                    c.left = points(&v)?;
                }
                if let Some(v) = arg(args, 1, "Top") {
                    c.top = points(&v)?;
                }
                if let Some(v) = arg(args, 2, "Width") {
                    c.width = points(&v)?;
                }
                if let Some(v) = arg(args, 3, "Height") {
                    c.height = points(&v)?;
                }
            }
            "zorder" | "dropdown" | "copy" | "cut" | "paste" | "repaint" | "redraw" => {}
            _ => return Err(ScriptError::runtime(438)),
        }
        Ok(())
    }
}

/// Members of a UserForm object besides its controls (lowercase).
const FORM_MEMBERS: &[&str] = &[
    "show",
    "hide",
    "controls",
    "caption",
    "name",
    "tag",
    "width",
    "height",
    "insidewidth",
    "insideheight",
    "left",
    "top",
    "backcolor",
    "forecolor",
    "bordercolor",
    "enabled",
    "visible",
    "startupposition",
    "activecontrol",
    "font",
    "picture",
    "mouseicon",
    "scrolltop",
    "scrollleft",
    "zoom",
    "repaint",
    "printform",
    "zorder",
    "move",
    "setfocus",
    "redoaction",
    "undoaction",
    "setdefaulttaborder",
];

fn text_col(c: &crate::forms::Control) -> usize {
    if c.text_column > 0 {
        c.text_column as usize - 1
    } else {
        0
    }
}

fn bound_value(c: &crate::forms::Control, s: &ControlState) -> Option<Value> {
    if s.list_index < 0 {
        return None;
    }
    if c.bound_column == 0 {
        return Some(Value::Long(s.list_index));
    }
    let row = s.list.get(s.list_index as usize)?;
    Some(match row.get((c.bound_column - 1) as usize) {
        Some(t) => Value::String(t.clone()),
        None => Value::Null,
    })
}

fn text_of(v: &Value) -> R<String> {
    if v.is_null() {
        return Ok(String::new());
    }
    convert::to_str(v)
}

/// `List` as a 0-based 2-D array.
fn list_array(list: &[Vec<String>], column_count: i32) -> Value {
    if list.is_empty() {
        return Value::Null;
    }
    let cols = list
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(column_count.max(1) as usize);
    let rows = list
        .iter()
        .map(|r| {
            (0..cols)
                .map(|j| r.get(j).map_or(Value::Null, |x| Value::String(x.clone())))
                .collect()
        })
        .collect();
    Value::array(Array::from_rows(0, rows))
}

/// Rows of strings from an array (1-D: one column; 2-D: rows × columns) or a single value.
pub(crate) fn rows_of(v: &Value) -> R<Vec<Vec<String>>> {
    match v {
        Value::Array(a) => match a.rank() {
            0 => Ok(Vec::new()),
            1 => Ok(a
                .values()
                .iter()
                .map(|x| vec![convert::display(x)])
                .collect()),
            2 => Ok(a
                .rows()
                .iter()
                .map(|r| r.iter().map(convert::display).collect())
                .collect()),
            _ => Err(ScriptError::runtime(13)),
        },
        Value::Empty | Value::Null => Ok(Vec::new()),
        other => Ok(vec![vec![convert::display(other)]]),
    }
}

// ---- Objects ----

/// A control on a form.
pub(crate) struct ControlObj {
    pub rt: Rc<FormRt>,
    pub i: usize,
}

impl Dispatch for ControlObj {
    fn type_name(&self) -> String {
        self.rt.st.borrow().design.controls[self.i]
            .kind
            .type_name()
            .to_owned()
    }

    fn get(&self, name: &str, args: &[Arg]) -> R<Value> {
        self.rt.ctl_get(self.i, name, args)
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> R<()> {
        self.rt.ctl_put(self.i, name, args, value)
    }

    fn call(&self, name: &str, args: &[Arg]) -> R<Value> {
        match self.rt.ctl_call(self.i, name, args) {
            Err(e) if e.number == 438 => self.get(name, args),
            Err(e) => Err(e),
            Ok(()) => Ok(Value::Empty),
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// `Controls`, `Frame.Controls`, `MultiPage.Pages`.
pub(crate) struct ControlsObj {
    pub rt: Rc<FormRt>,
    items: Vec<usize>,
    type_name: &'static str,
}

impl Dispatch for ControlsObj {
    fn type_name(&self) -> String {
        self.type_name.to_owned()
    }

    fn get(&self, name: &str, args: &[Arg]) -> R<Value> {
        if is(name, "count") {
            return Ok(Value::Long(self.items.len() as i32));
        }
        if name.is_empty() || is(name, "item") {
            let key = arg(args, 0, "varg").ok_or_else(|| ScriptError::runtime(449))?;
            let found = match &key {
                Value::String(s) => self
                    .items
                    .iter()
                    .copied()
                    .find(|&i| is(&self.rt.st.borrow().design.controls[i].name, s)),
                other => {
                    let n = convert::to_i64(other)?;
                    usize::try_from(n)
                        .ok()
                        .and_then(|n| self.items.get(n).copied())
                }
            };
            let i = found.ok_or_else(|| {
                ScriptError::new(-2147024809, "Could not find the specified object.")
            })?;
            return Ok(Value::object(self.rt.control(i)));
        }
        Err(ScriptError::no_member(self.type_name, name))
    }

    fn enumerate(&self) -> R<Vec<Value>> {
        Ok(self
            .items
            .iter()
            .map(|&i| Value::object(self.rt.control(i)))
            .collect())
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// `.Font` of a control (or of the form when `target` is `None`).
pub(crate) struct FontObj {
    rt: Rc<FormRt>,
    target: Option<usize>,
}

impl FontObj {
    fn with<T>(&self, f: impl FnOnce(&mut Font) -> T) -> T {
        let mut st = self.rt.st.borrow_mut();
        match self.target {
            Some(i) => f(&mut st.design.controls[i].font),
            None => f(&mut st.design.font),
        }
    }
}

impl Dispatch for FontObj {
    fn type_name(&self) -> String {
        "Font".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> R<Value> {
        let key = name.to_ascii_lowercase();
        self.with(|f| {
            Ok(match key.as_str() {
                "" | "name" => Value::String(f.name.clone()),
                "size" => Value::Currency((f64::from(f.size) * 10_000.0).round() as i64),
                "bold" => Value::Boolean(f.bold),
                "italic" => Value::Boolean(f.italic),
                "underline" => Value::Boolean(f.underline),
                "strikethrough" => Value::Boolean(f.strikeout),
                "charset" => Value::Integer(i16::from(f.charset)),
                "weight" => Value::Integer(if f.bold { 700 } else { 400 }),
                _ => return Err(ScriptError::no_member("Font", name)),
            })
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> R<()> {
        let key = name.to_ascii_lowercase();
        let text = convert::to_str(&value);
        let flag = convert::to_bool(&value);
        let size = convert::to_f64(&value);
        self.with(|f| {
            match key.as_str() {
                "" | "name" => f.name = text?,
                "size" => f.size = size? as f32,
                "bold" => f.bold = flag?,
                "italic" => f.italic = flag?,
                "underline" => f.underline = flag?,
                "strikethrough" => f.strikeout = flag?,
                "weight" => f.bold = size? >= 600.0,
                "charset" => {}
                _ => return Err(ScriptError::no_member("Font", name)),
            }
            Ok(())
        })
    }
}

/// `MSForms.ReturnBoolean` / `ReturnInteger`: an event argument the handler sets
/// (`Cancel = True`, `KeyAscii = 0`).
pub(crate) struct ReturnValue {
    value: RefCell<Value>,
    type_name: &'static str,
}

impl ReturnValue {
    fn new(value: Value, type_name: &'static str) -> Rc<ReturnValue> {
        Rc::new(ReturnValue {
            value: RefCell::new(value),
            type_name,
        })
    }

    fn get_value(&self) -> Value {
        self.value.borrow().clone()
    }
}

impl Dispatch for ReturnValue {
    fn type_name(&self) -> String {
        self.type_name.to_owned()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> R<Value> {
        if name.is_empty() || is(name, "value") {
            Ok(self.value.borrow().clone())
        } else {
            Err(ScriptError::no_member(self.type_name, name))
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> R<()> {
        if name.is_empty() || is(name, "value") {
            *self.value.borrow_mut() = value;
            Ok(())
        } else {
            Err(ScriptError::no_member(self.type_name, name))
        }
    }
}

/// The form a control or `Controls` object belongs to.
pub(crate) fn form_of_object(o: &ObjectRef) -> Option<Rc<FormRt>> {
    let any = o.as_any()?;
    if let Some(c) = any.downcast_ref::<ControlObj>() {
        return Some(c.rt.clone());
    }
    if let Some(c) = any.downcast_ref::<ControlsObj>() {
        return Some(c.rt.clone());
    }
    None
}

/// The form of a form instance.
pub(crate) fn form_of_instance(o: &ObjectRef) -> Option<Rc<FormRt>> {
    o.as_any()?.downcast_ref::<Instance>()?.form.clone()
}

// ---- The interpreter's side ----

impl Interp<'_> {
    /// A form module's default instance (`UserForm1.Show`), loaded on first use.
    pub(crate) fn default_form(&mut self, m: &Rc<Module>) -> R<ObjectRef> {
        if let Some(o) = self.st.default_forms.get(&m.idx) {
            return Ok(o.clone());
        }
        let (o, rt) = self.new_form(m);
        self.st.default_forms.insert(m.idx, o.clone());
        if let Err(e) = self.form_initialize(&o, &rt) {
            self.st.default_forms.remove(&m.idx);
            return Err(e);
        }
        Ok(o)
    }

    /// A new instance of a form module, not initialised yet.
    pub(crate) fn new_form(&mut self, m: &Rc<Module>) -> (ObjectRef, Rc<FormRt>) {
        let design = self
            .st
            .forms
            .get(&*m.key)
            .map(|d| (**d).clone())
            .unwrap_or_else(|| FormDesign::new(&m.name));
        let rt = FormRt::new(m.clone(), design);
        let fields = Rc::new(Scope::default());
        let obj: ObjectRef = Rc::new(Instance {
            module: m.clone(),
            fields,
            form: Some(rt.clone()),
        });
        rt.set_me(&obj);
        (obj, rt)
    }

    /// Module variables, lists from RowSource, values from ControlSource, then
    /// `UserForm_Initialize`.
    pub(crate) fn form_initialize(&mut self, o: &ObjectRef, rt: &Rc<FormRt>) -> R<()> {
        let inst = o
            .as_any()
            .and_then(|a| a.downcast_ref::<Instance>())
            .ok_or_else(|| ScriptError::runtime(424))?;
        let module = inst.module.clone();
        let fields = inst.fields.clone();
        {
            let mut frame = self.module_frame(&module);
            for d in &module.vars {
                let slot = self.make_var(d, &mut frame)?;
                fields.borrow_mut().insert(d.name.key.clone(), slot);
            }
        }
        let n = rt.st.borrow().design.controls.len();
        for i in 0..n {
            self.form_fill_rows(rt, i);
            let source = rt.st.borrow().design.controls[i].control_source.clone();
            if !source.is_empty()
                && let Some(v) = self.cell_value(&source)
            {
                rt.load_value(i, &v);
            }
        }
        self.form_fire(o, rt, None, "Initialize", Vec::new())?;
        self.form_drain(rt)
    }

    fn range(&mut self, address: &str) -> Option<ObjectRef> {
        let g = self.host.globals()?;
        let r = g.get("Range", &[Arg::new(address)]).ok().or_else(|| {
            // `Sheet!Name` where the name is the workbook's.
            let (_, name) = address.rsplit_once('!')?;
            g.get("Range", &[Arg::new(name)]).ok()
        })?;
        r.as_object().cloned()
    }

    fn cell_value(&mut self, address: &str) -> Option<Value> {
        let r = self.range(address)?;
        r.get("Value", &[]).ok()
    }

    /// A ListBox's or ComboBox's rows from its RowSource.
    fn form_fill_rows(&mut self, rt: &Rc<FormRt>, i: usize) {
        let source = rt.st.borrow().design.controls[i].row_source.clone();
        if source.is_empty() {
            return;
        }
        let rows = self
            .cell_value(&source)
            .and_then(|v| rows_of(&v).ok())
            .unwrap_or_default();
        rt.set_rows(i, rows);
    }

    /// Runs `<control>_<event>` (or `UserForm_<event>`) if the module has it. Arguments beyond
    /// the handler's parameters are dropped. Returns whether there was a handler.
    pub(crate) fn form_fire(
        &mut self,
        o: &ObjectRef,
        rt: &Rc<FormRt>,
        control: Option<usize>,
        event: &str,
        mut args: Vec<ArgIn>,
    ) -> R<bool> {
        let owner = match control {
            Some(i) => rt.st.borrow().design.controls[i].name.clone(),
            None => "UserForm".to_owned(),
        };
        let key = format!("{owner}_{event}").to_lowercase();
        let module = rt.module.clone();
        let Some(p) = module.procs.get(key.as_str()).and_then(|g| g.sub.clone()) else {
            return Ok(false);
        };
        let Some(inst) = o.as_any().and_then(|a| a.downcast_ref::<Instance>()) else {
            return Ok(false);
        };
        let fields = inst.fields.clone();
        args.truncate(p.params.len());
        self.call_proc(&module, &p, args, Some((o.clone(), fields)), None)?;
        Ok(true)
    }

    /// Fires an event with a `Cancel As MSForms.ReturnBoolean` argument; true when cancelled.
    fn form_fire_cancel(
        &mut self,
        o: &ObjectRef,
        rt: &Rc<FormRt>,
        control: Option<usize>,
        event: &str,
    ) -> R<bool> {
        let cancel = ReturnValue::new(Value::Boolean(false), "ReturnBoolean");
        let arg: ObjectRef = cancel.clone();
        self.form_fire(
            o,
            rt,
            control,
            event,
            vec![ArgIn::value(Value::object(arg))],
        )?;
        Ok(convert::to_bool(&cancel.get_value()).unwrap_or(false))
    }

    /// Runs the events queued by state changes, in order.
    pub(crate) fn form_drain(&mut self, rt: &Rc<FormRt>) -> R<()> {
        let mut budget = 100_000u32;
        while let Some(p) = rt.take_pending() {
            budget -= 1;
            if budget == 0 {
                return Err(ScriptError::runtime(28));
            }
            let Some(o) = rt.me() else {
                continue;
            };
            match p {
                Pending::Event(Some(i), "\u{1}RowSource") => self.form_fill_rows(rt, i),
                Pending::Event(control, event) => {
                    self.form_fire(&o, rt, control, event, Vec::new())?;
                }
                Pending::Store(i) => {
                    let source = rt.st.borrow().design.controls[i].control_source.clone();
                    let value = rt.value(i);
                    if let Some(r) = self.range(&source) {
                        r.put("Value", &[], value)?;
                    }
                }
                Pending::Focus(i) => {
                    self.form_focus(&o, rt, i)?;
                }
            }
        }
        Ok(())
    }

    /// Drains the events of the form a control object belongs to.
    pub(crate) fn form_after(&mut self, o: &ObjectRef) -> R<()> {
        match form_of_object(o) {
            Some(rt) => self.form_drain(&rt),
            None => Ok(()),
        }
    }

    /// A member of a form instance that isn't one of its module's own: `Show`, `Hide`,
    /// controls, form properties.
    pub(crate) fn form_member(
        &mut self,
        o: &ObjectRef,
        rt: &Rc<FormRt>,
        key: &str,
        args: Vec<ArgIn>,
        _access: Access,
    ) -> R<Value> {
        let args: Vec<Arg> = args
            .into_iter()
            .map(|a| Arg {
                name: a.name.map(|n| n.to_string()),
                value: a.value,
            })
            .collect();
        match key {
            "show" => {
                let modal = match arg(&args, 0, "Modal") {
                    Some(v) => convert::to_i64(&self.deref(v)?)? != 0,
                    None => true,
                };
                self.form_show(o, rt, modal)?;
                Ok(Value::Empty)
            }
            "hide" => {
                rt.st.borrow_mut().visible = false;
                Ok(Value::Empty)
            }
            "setfocus" | "repaint" | "printform" | "zorder" | "redoaction" | "undoaction"
            | "setdefaulttaborder" => Ok(Value::Empty),
            "move" => {
                if let Some(v) = arg(&args, 0, "Left") {
                    rt.form_put("left", &[], v)?;
                }
                if let Some(v) = arg(&args, 1, "Top") {
                    rt.form_put("top", &[], v)?;
                }
                if let Some(v) = arg(&args, 2, "Width") {
                    rt.form_put("width", &[], v)?;
                }
                if let Some(v) = arg(&args, 3, "Height") {
                    rt.form_put("height", &[], v)?;
                }
                Ok(Value::Empty)
            }
            _ => {
                let v = rt.form_get(key, &args)?;
                self.form_drain(rt)?;
                Ok(v)
            }
        }
    }

    pub(crate) fn form_put(
        &mut self,
        rt: &Rc<FormRt>,
        key: &str,
        args: Vec<Arg>,
        value: Value,
    ) -> R<()> {
        if let Some(i) = rt.control_index(key) {
            // `Me.TextBox1 = "x"`: the control's default member.
            let c = rt.control(i);
            c.put("", &args, value)?;
        } else if key.is_empty() {
            return Err(ScriptError::runtime(438));
        } else {
            rt.form_put(key, &args, value)?;
        }
        self.form_drain(rt)
    }

    /// `Load UserForm1`, or the first use of a form: makes sure it's loaded.
    pub(crate) fn form_load(&mut self, o: &ObjectRef, rt: &Rc<FormRt>) -> R<()> {
        if rt.st.borrow().loaded {
            return Ok(());
        }
        rt.reset();
        self.form_initialize(o, rt)
    }

    /// `form.Show`: modal forms run their loop here; modeless ones after the macro.
    pub(crate) fn form_show(&mut self, o: &ObjectRef, rt: &Rc<FormRt>, modal: bool) -> R<()> {
        self.form_load(o, rt)?;
        {
            let st = rt.st.borrow();
            if st.visible {
                if modal && !st.modal {
                    return Err(ScriptError::new(
                        400,
                        "Form already displayed; can't show modally",
                    ));
                }
                return Ok(());
            }
        }
        {
            let mut st = rt.st.borrow_mut();
            st.visible = true;
            st.modal = modal;
        }
        rt.focus_first();
        self.form_fire(o, rt, None, "Activate", Vec::new())?;
        self.form_drain(rt)?;
        if !modal {
            if !self.st.modeless.iter().any(|m| Value::same_object(m, o)) {
                self.st.modeless.push(o.clone());
            }
            return Ok(());
        }
        self.form_loop(o, rt)
    }

    /// Shows the form through the host until it's hidden or unloaded.
    pub(crate) fn form_loop(&mut self, o: &ObjectRef, rt: &Rc<FormRt>) -> R<()> {
        let name = rt.name();
        let r = (|| {
            loop {
                {
                    let st = rt.st.borrow();
                    if !st.loaded || !st.visible {
                        return Ok(());
                    }
                }
                let view = rt.view();
                let input = self.host.form_event(&view)?;
                self.form_input(o, rt, input)?;
            }
        })();
        self.host.form_closed(&name);
        if r.is_err() {
            rt.st.borrow_mut().visible = false;
        }
        r
    }

    /// Applies the user's step and runs its events.
    pub(crate) fn form_input(&mut self, o: &ObjectRef, rt: &Rc<FormRt>, input: FormInput) -> R<()> {
        for e in &input.edits {
            rt.apply_edit(e)?;
            self.form_drain(rt)?;
        }
        let n = rt.st.borrow().design.controls.len();
        let valid = |i: usize| {
            if i < n {
                Ok(i)
            } else {
                Err(ScriptError::runtime(9))
            }
        };
        match input.action {
            Action::None => {}
            Action::Click(i) => {
                let i = valid(i)?;
                let kind = rt.st.borrow().design.controls[i].kind;
                if focusable(kind) && !has_value(kind) && !self.form_focus(o, rt, i)? {
                    return Ok(());
                }
                self.form_fire(o, rt, Some(i), "Click", Vec::new())?;
            }
            Action::DblClick(i) => {
                let i = valid(i)?;
                self.form_fire_cancel(o, rt, Some(i), "DblClick")?;
            }
            Action::FormClick => {
                self.form_fire(o, rt, None, "Click", Vec::new())?;
            }
            Action::Focus(i) => {
                let i = valid(i)?;
                self.form_focus(o, rt, i)?;
            }
            action @ (Action::DefaultKey | Action::CancelKey) => {
                let view = rt.view();
                let button = if action == Action::DefaultKey {
                    view.default_button()
                } else {
                    view.cancel_button()
                };
                if let Some(b) = button
                    && view.design.controls[b].enabled
                    && view.shown(b)
                {
                    self.form_fire(o, rt, Some(b), "Click", Vec::new())?;
                }
            }
            Action::KeyPress { control, ch } => {
                let i = valid(control)?;
                let key = ReturnValue::new(Value::Integer(ch as u32 as i16), "ReturnInteger");
                let arg: ObjectRef = key.clone();
                self.form_fire(
                    o,
                    rt,
                    Some(i),
                    "KeyPress",
                    vec![ArgIn::value(Value::object(arg))],
                )?;
                let code = convert::to_i64(&key.get_value()).unwrap_or(0);
                if code > 0
                    && let Some(c) = char::from_u32(code as u32)
                    && !c.is_control()
                {
                    let (mut text, max, kind) = {
                        let st = rt.st.borrow();
                        let c = &st.design.controls[i];
                        (st.state[i].text.clone(), c.max_length, c.kind)
                    };
                    if matches!(kind, ControlKind::TextBox | ControlKind::ComboBox)
                        && (max == 0 || (text.chars().count() as u32) < max)
                    {
                        text.push(c);
                        rt.set_text(i, text);
                    }
                }
            }
            Action::KeyDown {
                control,
                key,
                shift,
            } => {
                let i = valid(control)?;
                let code = ReturnValue::new(Value::Integer(key as i16), "ReturnInteger");
                let arg: ObjectRef = code;
                self.form_fire(
                    o,
                    rt,
                    Some(i),
                    "KeyDown",
                    vec![
                        ArgIn::value(Value::object(arg)),
                        ArgIn::value(Value::Integer(shift as i16)),
                    ],
                )?;
            }
            Action::Close => {
                self.form_unload(o, rt, 0)?;
            }
        }
        self.form_drain(rt)
    }

    /// Moves the focus to `to`: BeforeUpdate / AfterUpdate and Exit of the control losing it,
    /// Enter of `to`. False when a handler cancelled.
    pub(crate) fn form_focus(&mut self, o: &ObjectRef, rt: &Rc<FormRt>, to: usize) -> R<bool> {
        let from = rt.st.borrow().focus;
        if from == Some(to) {
            return Ok(true);
        }
        if let Some(f) = from {
            let dirty = rt.st.borrow().dirty.get(f).copied().unwrap_or(false);
            if dirty {
                if self.form_fire_cancel(o, rt, Some(f), "BeforeUpdate")? {
                    return Ok(false);
                }
                rt.st.borrow_mut().dirty[f] = false;
                self.form_fire(o, rt, Some(f), "AfterUpdate", Vec::new())?;
            }
            if self.form_fire_cancel(o, rt, Some(f), "Exit")? {
                return Ok(false);
            }
        }
        {
            let mut st = rt.st.borrow_mut();
            st.focus = Some(to);
            if let Some(d) = st.dirty.get_mut(to) {
                *d = false;
            }
        }
        self.form_fire(o, rt, Some(to), "Enter", Vec::new())?;
        Ok(true)
    }

    /// `UserForm_QueryClose`; true when cancelled.
    fn form_query_close(&mut self, o: &ObjectRef, rt: &Rc<FormRt>, mode: i16) -> R<bool> {
        let cancel = new_slot(Value::Integer(0), Ty::Integer);
        let close_mode = new_slot(Value::Integer(mode), Ty::Integer);
        let args = vec![
            ArgIn {
                name: None,
                value: Value::Integer(0),
                place: Some(Place::from_slot(cancel.clone())),
            },
            ArgIn {
                name: None,
                value: Value::Integer(mode),
                place: Some(Place::from_slot(close_mode)),
            },
        ];
        self.form_fire(o, rt, None, "QueryClose", args)?;
        let v = cancel.borrow().value.clone();
        Ok(convert::to_bool(&v).unwrap_or(false))
    }

    /// `Unload form` (mode 1), the close box (mode 0): QueryClose (which can cancel), then
    /// Terminate; the default instance is forgotten, so the next use loads a fresh one.
    pub(crate) fn form_unload(&mut self, o: &ObjectRef, rt: &Rc<FormRt>, mode: i16) -> R<()> {
        if !rt.st.borrow().loaded {
            return Ok(());
        }
        if self.form_query_close(o, rt, mode)? {
            return Ok(());
        }
        {
            let mut st = rt.st.borrow_mut();
            st.loaded = false;
            st.visible = false;
            st.pending.clear();
        }
        self.form_fire(o, rt, None, "Terminate", Vec::new())?;
        let idx = rt.module.idx;
        if self
            .st
            .default_forms
            .get(&idx)
            .is_some_and(|d| Value::same_object(d, o))
        {
            self.st.default_forms.remove(&idx);
        }
        self.st.modeless.retain(|m| !Value::same_object(m, o));
        Ok(())
    }

    /// The `Load` statement.
    pub(crate) fn exec_load(&mut self, v: Value) -> R<()> {
        let o = v
            .as_object()
            .cloned()
            .ok_or_else(|| ScriptError::runtime(424))?;
        let rt = form_of_instance(&o).ok_or_else(|| ScriptError::runtime(424))?;
        self.form_load(&o, &rt)
    }

    /// The `Unload` statement (a form, or a control: controls added at run time only).
    pub(crate) fn exec_unload(&mut self, v: Value) -> R<()> {
        let o = v
            .as_object()
            .cloned()
            .ok_or_else(|| ScriptError::runtime(424))?;
        match form_of_instance(&o) {
            Some(rt) => self.form_unload(&o, &rt, 1),
            None => Err(ScriptError::new(
                365,
                "Unable to unload within this context",
            )),
        }
    }

    /// Runs the loops of modeless forms still shown when a macro ends.
    pub(crate) fn run_modeless(&mut self) -> R<()> {
        while let Some(o) = self.st.modeless.first().cloned() {
            let Some(rt) = form_of_instance(&o) else {
                self.st.modeless.remove(0);
                continue;
            };
            let r = self.form_loop(&o, &rt);
            self.st.modeless.retain(|m| !Value::same_object(m, &o));
            r?;
        }
        Ok(())
    }
}
