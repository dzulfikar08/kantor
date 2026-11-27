//! Writes a `vbaProject.bin` from module sources in a folder (for making
//! test workbooks by hand): `NAME.bas` standard modules, `NAME.cls` class
//! modules, `NAME.doc` document modules (`ThisWorkbook`, `Sheet1`) and
//! `NAME.frm` UserForms. A form file starts with its design, one line each,
//! fields split by `|`, then a line `---` and the form's code:
//!
//! ```text
//! Form|Caption=Order|Width=300|Height=200
//! Control|TextBox|txtName|80|12|120|18|Value=Ann
//! Control|OptionButton|optAir|6|6|80|18|Parent=fraShip|Caption=Air
//! ---
//! Private Sub cmdOK_Click() ...
//! ```
//!
//! `cargo run -p kantor_vba --example vba_build -- DIR OUT.bin`

use std::path::Path;

use kantor_vba::ModuleKind;
use kantor_vba::forms::{Control, ControlKind, FormDesign};
use kantor_vba::synth::Builder;

fn kind(s: &str) -> ControlKind {
    match s {
        "Label" => ControlKind::Label,
        "TextBox" => ControlKind::TextBox,
        "CommandButton" => ControlKind::CommandButton,
        "CheckBox" => ControlKind::CheckBox,
        "OptionButton" => ControlKind::OptionButton,
        "ToggleButton" => ControlKind::ToggleButton,
        "ComboBox" => ControlKind::ComboBox,
        "ListBox" => ControlKind::ListBox,
        "Frame" => ControlKind::Frame,
        "MultiPage" => ControlKind::MultiPage,
        "Page" => ControlKind::Page,
        "Image" => ControlKind::Image,
        "SpinButton" => ControlKind::SpinButton,
        "ScrollBar" => ControlKind::ScrollBar,
        "TabStrip" => ControlKind::TabStrip,
        other => panic!("unknown control kind {other}"),
    }
}

fn flag(v: &str) -> bool {
    matches!(v, "1" | "true" | "True")
}

fn num(v: &str) -> f32 {
    v.parse().unwrap_or_else(|_| panic!("not a number: {v}"))
}

fn form(name: &str, text: &str) -> (FormDesign, String) {
    let text = text.replace("\r\n", "\n");
    let (design, code) = text.split_once("\n---\n").expect("a form file needs a --- line");
    let mut f = FormDesign::new(name);
    for line in design.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split('|').collect();
        match parts[0] {
            "Form" => {
                for kv in &parts[1..] {
                    let (k, v) = kv.split_once('=').expect("Key=Value");
                    match k {
                        "Caption" => f.caption = v.into(),
                        "Width" => f.width = num(v),
                        "Height" => f.height = num(v),
                        "BackColor" => f.back_color = u32::from_str_radix(v, 16).unwrap(),
                        "StartUpPosition" => f.start_up_position = num(v) as i32,
                        "FontSize" => f.font.size = num(v),
                        _ => panic!("unknown form key {k}"),
                    }
                }
            }
            "Control" => {
                let mut c = Control::new(parts[2], kind(parts[1]));
                c.left = num(parts[3]);
                c.top = num(parts[4]);
                c.width = num(parts[5]);
                c.height = num(parts[6]);
                for kv in &parts[7..] {
                    let (k, v) = kv.split_once('=').expect("Key=Value");
                    match k {
                        "Caption" => c.caption = v.into(),
                        "Value" => c.value = v.into(),
                        "Parent" => {
                            c.parent = Some(
                                f.controls
                                    .iter()
                                    .position(|p| p.name == v)
                                    .unwrap_or_else(|| panic!("no parent {v}")),
                            )
                        }
                        "Default" => c.default = flag(v),
                        "Cancel" => c.cancel = flag(v),
                        "Enabled" => c.enabled = flag(v),
                        "Locked" => c.locked = flag(v),
                        "MultiLine" => c.multi_line = flag(v),
                        "PasswordChar" => c.password_char = v.chars().next(),
                        "MaxLength" => c.max_length = num(v) as u32,
                        "RowSource" => c.row_source = v.into(),
                        "ControlSource" => c.control_source = v.into(),
                        "GroupName" => c.group_name = v.into(),
                        "ColumnCount" => c.column_count = num(v) as i32,
                        "MultiSelect" => c.multi_select = num(v) as u8,
                        "ListStyle" => c.list_style = num(v) as u8,
                        "DropList" => c.drop_list = flag(v),
                        "Min" => c.min = num(v) as i32,
                        "Max" => c.max = num(v) as i32,
                        "Position" => c.position = num(v) as i32,
                        "Tabs" => c.tabs = v.split(',').map(str::to_owned).collect(),
                        "TabIndex" => c.tab_index = num(v) as i16,
                        "FontSize" => c.font.size = num(v),
                        "Bold" => c.font.bold = flag(v),
                        "BackColor" => c.back_color = u32::from_str_radix(v, 16).unwrap(),
                        "ForeColor" => c.fore_color = u32::from_str_radix(v, 16).unwrap(),
                        "Tip" => c.tip = v.into(),
                        _ => panic!("unknown control key {k}"),
                    }
                }
                c.id = f.controls.len() as i32 + 1;
                if c.tab_index < 0 {
                    c.tab_index = f.controls.len() as i16;
                }
                f.controls.push(c);
            }
            other => panic!("unknown line {other}"),
        }
    }
    (f, code.to_owned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, dir, out] = &args[..] else {
        eprintln!("usage: vba_build DIR OUT.bin");
        std::process::exit(2);
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").path())
        .collect();
    files.sort();
    // ThisWorkbook and the sheets first, as Excel lists them.
    files.sort_by_key(|p| p.extension().is_none_or(|e| e != "doc"));
    let mut b = Builder::new(1252);
    for path in &files {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(path).expect("read module");
        b = match ext {
            "bas" => b.module(name, ModuleKind::Standard, &text),
            "cls" => b.module(name, ModuleKind::Class, &text),
            "doc" => b.module(name, ModuleKind::Document, &text),
            "frm" => {
                let (design, code) = form(name, &text);
                b.form(&design, &code)
            }
            _ => continue,
        };
    }
    let bytes = b.build().expect("build project");
    std::fs::write(Path::new(out), bytes).expect("write");
}
