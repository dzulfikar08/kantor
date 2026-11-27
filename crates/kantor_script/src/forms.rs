//! UserForms between a script and the host that shows them.
//!
//! A form's design comes from the file ([`kantor_vba::forms::FormDesign`]: controls, positions,
//! captions, colours). While a script shows a form, the interpreter owns its state (control
//! values, list items, what has the focus) and hands the host a [`FormView`] — everything needed
//! to draw it — through [`crate::Host::form_event`]. The host shows or updates the form, waits
//! for the user, and returns a [`FormInput`]: the values the user changed ([`Edit`]) and what
//! happened ([`Action`]: a click, a focus change, a key, the close box). The interpreter applies
//! it, runs the event handlers (`CommandButton1_Click`, `TextBox1_Change`, `UserForm_QueryClose`
//! …), and asks again with the new view until the form is hidden or unloaded; then it calls
//! [`crate::Host::form_closed`].
//!
//! Everything here is plain data (`Send`): a host may draw on another thread than the one
//! running the script.

pub use kantor_vba::forms::{
    Control, ControlKind, Font, FormDesign, Picture, PictureFormat, ole_rgb, rgb_ole,
};

/// The run-time state of one control (the design has the rest).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlState {
    /// A TextBox's or ComboBox's text.
    pub text: String,
    /// A CheckBox, OptionButton or ToggleButton: `None` is Null (neither).
    pub checked: Option<bool>,
    /// A ScrollBar's or SpinButton's value; a MultiPage's or TabStrip's selected page.
    pub number: i32,
    /// A ListBox's or ComboBox's rows (each with its columns).
    pub list: Vec<Vec<String>>,
    /// The selected row, -1 for none.
    pub list_index: i32,
    /// A multi-select ListBox's selected rows.
    pub selected: Vec<bool>,
}

/// A shown form: its design as the script has changed it (captions, positions, colours,
/// visibility) and its controls' state.
#[derive(Clone, Debug, PartialEq)]
pub struct FormView {
    pub design: FormDesign,
    /// One per control of `design.controls`.
    pub state: Vec<ControlState>,
    /// The control with the focus.
    pub focus: Option<usize>,
    /// Controls whose every change the script wants right away (they have `Change`,
    /// `KeyPress` or `KeyDown` handlers): send each edit of them as it happens.
    pub live: Vec<usize>,
    /// Controls with a `KeyPress` handler: send typed characters as [`Action::KeyPress`]
    /// instead of inserting them; the script decides what goes in.
    pub key_press: Vec<usize>,
    /// Whether the script waits for the form (`Show`, `Show vbModal`).
    pub modal: bool,
    /// Where the script put the form (`Left`, `Top`, in points from the screen's top left);
    /// `None` leaves it to the host (centred on the owner).
    pub position: Option<(f32, f32)>,
}

impl FormView {
    /// The index of the control called `name` (any case).
    pub fn control(&self, name: &str) -> Option<usize> {
        self.design
            .controls
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The CommandButton Enter clicks.
    pub fn default_button(&self) -> Option<usize> {
        self.design
            .controls
            .iter()
            .position(|c| c.default && c.kind == ControlKind::CommandButton)
    }

    /// The CommandButton Esc clicks.
    pub fn cancel_button(&self) -> Option<usize> {
        self.design
            .controls
            .iter()
            .position(|c| c.cancel && c.kind == ControlKind::CommandButton)
    }

    /// Whether control `i` and all its parents are visible.
    pub fn shown(&self, i: usize) -> bool {
        let mut at = Some(i);
        while let Some(c) = at {
            let Some(control) = self.design.controls.get(c) else {
                return false;
            };
            if !control.visible {
                return false;
            }
            // A Page shows when it's its MultiPage's selected page.
            if control.kind == ControlKind::Page
                && let Some(mp) = control.parent
            {
                let pages = self.design.children(Some(mp));
                let selected = self.state.get(mp).map_or(0, |s| s.number);
                if pages.get(selected.max(0) as usize) != Some(&c) {
                    return false;
                }
            }
            at = control.parent;
        }
        true
    }

    /// The text a list row shows in a ComboBox's edit part or for `Text`: the TextColumn,
    /// or the first column.
    pub fn row_text(&self, i: usize, row: usize) -> String {
        let c = &self.design.controls[i];
        let col = if c.text_column > 0 {
            c.text_column as usize - 1
        } else {
            0
        };
        self.state[i]
            .list
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_default()
    }
}

/// A value the user changed.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A TextBox's or ComboBox's typed text.
    Text { control: usize, text: String },
    /// A CheckBox, OptionButton or ToggleButton.
    Checked {
        control: usize,
        checked: Option<bool>,
    },
    /// The row picked in a ListBox or ComboBox (-1: none).
    ListIndex { control: usize, index: i32 },
    /// A row of a multi-select ListBox selected or not.
    Selected {
        control: usize,
        row: usize,
        selected: bool,
    },
    /// A ScrollBar's or SpinButton's value, a MultiPage's or TabStrip's page.
    Number { control: usize, value: i32 },
}

impl Edit {
    pub fn control(&self) -> usize {
        match self {
            Edit::Text { control, .. }
            | Edit::Checked { control, .. }
            | Edit::ListIndex { control, .. }
            | Edit::Selected { control, .. }
            | Edit::Number { control, .. } => *control,
        }
    }
}

/// What the user did.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Action {
    /// Only edits (a live control changed).
    #[default]
    None,
    /// A control without a value was clicked: a CommandButton, Label, Image or Frame. (A
    /// click that changes a CheckBox, OptionButton, ToggleButton or list is an [`Edit`]; its
    /// `Click` event follows from the change.)
    Click(usize),
    DblClick(usize),
    /// The form's own background was clicked.
    FormClick,
    /// The focus moves to a control (Tab, a click into it).
    Focus(usize),
    /// Enter outside a multi-line TextBox: the Default button.
    DefaultKey,
    /// Esc: the Cancel button.
    CancelKey,
    /// A character typed into a control with a `KeyPress` handler.
    KeyPress {
        control: usize,
        ch: char,
    },
    /// A key went down in a control with a `KeyDown` handler (`KeyCode`, `Shift` as VBA
    /// gives them: 13 Enter, 27 Esc, 37–40 arrows; Shift 1, Ctrl 2, Alt 4).
    KeyDown {
        control: usize,
        key: i32,
        shift: i32,
    },
    /// The close box (or Alt+F4): `UserForm_QueryClose` with CloseMode 0.
    Close,
}

/// The user's next step on a shown form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormInput {
    pub edits: Vec<Edit>,
    pub action: Action,
}

impl FormInput {
    pub fn action(action: Action) -> FormInput {
        FormInput {
            edits: Vec::new(),
            action,
        }
    }
}

/// A value to put in a control by name, for scripted answers (tests, `--form-answers`).
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Text(String),
    Bool(bool),
    Number(f64),
    /// Several rows of a multi-select ListBox, by their text.
    Rows(Vec<String>),
}

/// Turns "set these controls, then click that one" into a [`FormInput`] for `view`. A
/// ListBox or ComboBox takes a row number or the text of a row; a CheckBox, OptionButton or
/// ToggleButton a Boolean; a ScrollBar, SpinButton or MultiPage a number. `click` names a
/// control, or is `"close"` for the close box. Fails with a message naming what doesn't fit.
pub fn scripted_input(
    view: &FormView,
    set: &[(String, Answer)],
    click: Option<&str>,
) -> Result<FormInput, String> {
    let mut edits = Vec::new();
    for (name, answer) in set {
        let i = view
            .control(name)
            .ok_or_else(|| format!("{} has no control called {name}", view.design.name))?;
        let c = &view.design.controls[i];
        let st = &view.state[i];
        let find_row = |text: &str| {
            st.list
                .iter()
                .position(|r| r.iter().any(|cell| cell == text))
                .map(|r| r as i32)
        };
        let edit = match (c.kind, answer) {
            (ControlKind::TextBox, a) => Edit::Text {
                control: i,
                text: answer_text(a),
            },
            (ControlKind::ComboBox, Answer::Number(n)) => Edit::ListIndex {
                control: i,
                index: *n as i32,
            },
            (ControlKind::ComboBox, a) => {
                let text = answer_text(a);
                match find_row(&text) {
                    Some(index) => Edit::ListIndex { control: i, index },
                    None => Edit::Text { control: i, text },
                }
            }
            (ControlKind::ListBox, Answer::Number(n)) => Edit::ListIndex {
                control: i,
                index: *n as i32,
            },
            (ControlKind::ListBox, Answer::Rows(rows)) => {
                for r in rows {
                    let row = find_row(r).ok_or_else(|| format!("{name} has no row {r:?}"))?;
                    edits.push(Edit::Selected {
                        control: i,
                        row: row as usize,
                        selected: true,
                    });
                }
                continue;
            }
            (ControlKind::ListBox, a) => {
                let text = answer_text(a);
                let index = find_row(&text).ok_or_else(|| format!("{name} has no row {text:?}"))?;
                Edit::ListIndex { control: i, index }
            }
            (
                ControlKind::CheckBox | ControlKind::OptionButton | ControlKind::ToggleButton,
                Answer::Bool(b),
            ) => Edit::Checked {
                control: i,
                checked: Some(*b),
            },
            (
                ControlKind::ScrollBar
                | ControlKind::SpinButton
                | ControlKind::MultiPage
                | ControlKind::TabStrip,
                Answer::Number(n),
            ) => Edit::Number {
                control: i,
                value: *n as i32,
            },
            (kind, a) => {
                return Err(format!(
                    "{name} is a {} and can't take {a:?}",
                    kind.type_name()
                ));
            }
        };
        edits.push(edit);
    }
    let action = match click {
        None => Action::None,
        Some(n) if n.eq_ignore_ascii_case("close") && view.control(n).is_none() => Action::Close,
        Some(n) => Action::Click(
            view.control(n)
                .ok_or_else(|| format!("{} has no control called {n}", view.design.name))?,
        ),
    };
    Ok(FormInput { edits, action })
}

fn answer_text(a: &Answer) -> String {
    match a {
        Answer::Text(s) => s.clone(),
        Answer::Bool(b) => if *b { "True" } else { "False" }.to_owned(),
        Answer::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        Answer::Rows(r) => r.join(", "),
    }
}
