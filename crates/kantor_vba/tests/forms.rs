//! UserForm designers: two real Excel-made forms (see
//! tests/fixtures/README.md) and forms written by `synth::Builder::form`.

use kantor_vba::forms::{Control, ControlKind, Font, FormDesign, Picture, PictureFormat, ole_rgb};
use kantor_vba::synth::Builder;
use kantor_vba::{ModuleKind, VbaProject};

const HRIDAY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hriday-userform.vbaProject.bin"
));
const STUDENT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ayliinkaya-StudentRegistration.vbaProject.bin"
));

#[test]
fn excel_form_with_labels_text_boxes_and_buttons() {
    let p = VbaProject::read(HRIDAY).unwrap();
    assert_eq!(p.module("UserForm4").unwrap().kind, ModuleKind::Form);
    let f = p.form("userform4").unwrap().unwrap();
    assert_eq!(f.name, "UserForm4");
    assert_eq!(f.caption, "UserForm4");
    assert_eq!((f.width, f.height), (491.5, 408.5));
    assert_eq!(ole_rgb(f.back_color), 0xFFC0C0);
    assert_eq!(f.start_up_position, 1);
    let names: Vec<(&str, ControlKind)> = f
        .controls
        .iter()
        .map(|c| (c.name.as_str(), c.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("Label1", ControlKind::Label),
            ("TextBox1", ControlKind::TextBox),
            ("Label2", ControlKind::Label),
            ("TextBox2", ControlKind::TextBox),
            ("Label3", ControlKind::Label),
            ("TextBox3", ControlKind::TextBox),
            ("ADD", ControlKind::CommandButton),
            ("RESET", ControlKind::CommandButton),
        ]
    );
    let l = f.control("label1").unwrap();
    assert_eq!(l.caption, "NAME");
    assert_eq!((l.left, l.top, l.width), (36.0, 18.0, 126.0));
    assert_eq!(l.font.name, "Yu Gothic UI");
    assert_eq!(l.font.size, 16.0);
    assert!(l.font.bold);
    assert_eq!(l.text_align, 3);
    assert_eq!(ole_rgb(l.fore_color), 0xFFFFFF);
    assert!(!l.tab_stop);
    let t = f.control("TextBox2").unwrap();
    assert_eq!(t.tab_index, 3);
    assert!(t.tab_stop && t.visible && t.enabled && !t.multi_line);
    assert_eq!(t.font.name, "Tahoma");
    let add = f.control("ADD").unwrap();
    assert_eq!(add.caption, "ADD");
    assert_eq!(ole_rgb(add.back_color), 0xC0C000);
    assert_eq!(add.parent, None);
    assert_eq!(p.forms().len(), 1);
}

#[test]
fn excel_form_with_combo_boxes_and_unicode_captions() {
    let p = VbaProject::read(STUDENT).unwrap();
    assert_eq!(p.codepage, 1254);
    let f = p.form("KayitFormu").unwrap().unwrap();
    assert_eq!(f.controls.len(), 15);
    assert_eq!(f.back_color, 0x8000_0002);
    let title = f.control("Label5").unwrap();
    assert_eq!(title.caption, "ÖĞRENCİ KAYIT FORMU ");
    assert_eq!(title.font.size, 24.0);
    assert_eq!(title.font.charset, 162);
    let combo = f.control("ComboBox2").unwrap();
    assert_eq!(combo.kind, ControlKind::ComboBox);
    assert_eq!(combo.row_source, "VERİ!LABNO");
    assert_eq!(combo.value, "Laboratuvar Seçiniz");
    assert!(!combo.drop_list);
    assert_eq!(f.control("CommandButton2").unwrap().caption, "TEMİZLE");
    assert_eq!(f.control("CommandButton4").unwrap().tab_index, 12);
    assert_eq!(f.control("TextBox1").unwrap().font.size, 7.8);
}

/// A form with every kind of control the reader knows, positions in
/// multiples of 18 pt (exact in HIMETRIC).
fn every_control() -> FormDesign {
    let mut f = FormDesign::new("UserForm1");
    f.caption = "Order entry".into();
    f.width = 360.0;
    f.height = 270.0;
    f.back_color = 0x00DDEEFF;
    f.start_up_position = 2;
    f.font = Font {
        name: "Arial".into(),
        size: 10.0,
        bold: true,
        ..Font::default()
    };
    let mut add = |c: Control| {
        let id = f.controls.len() as i32 + 1;
        f.controls.push(Control { id, ..c });
        f.controls.len() - 1
    };
    let mut label = Control::new("lblName", ControlKind::Label);
    label.caption = "Ünïcödé name ✓".into();
    label.accelerator = Some('N');
    label.left = 18.0;
    label.top = 18.0;
    label.tab_index = 0;
    add(label);
    let mut text = Control::new("txtName", ControlKind::TextBox);
    text.value = "Ann".into();
    text.max_length = 10;
    text.password_char = Some('*');
    text.multi_line = true;
    text.locked = true;
    text.tag = "name".into();
    text.tip = "Your name".into();
    text.control_source = "Sheet1!A1".into();
    text.left = 108.0;
    text.top = 18.0;
    text.width = 144.0;
    text.tab_index = 1;
    add(text);
    let mut ok = Control::new("cmdOK", ControlKind::CommandButton);
    ok.caption = "OK".into();
    ok.default = true;
    ok.accelerator = Some('O');
    ok.back_color = 0x0000FF00;
    ok.text_align = 3;
    ok.tab_index = 2;
    add(ok);
    let mut cancel = Control::new("cmdCancel", ControlKind::CommandButton);
    cancel.caption = "Cancel".into();
    cancel.cancel = true;
    cancel.tab_index = 3;
    cancel.left = 90.0;
    add(cancel);
    let mut check = Control::new("chkRush", ControlKind::CheckBox);
    check.caption = "Rush".into();
    check.value = "1".into();
    check.caption_left = true;
    add(check);
    let mut list = Control::new("lstItems", ControlKind::ListBox);
    list.row_source = "Items!A1:B5".into();
    list.column_count = 2;
    list.bound_column = 2;
    list.multi_select = 1;
    list.list_style = 1;
    list.column_heads = true;
    list.height = 90.0;
    add(list);
    let mut combo = Control::new("cboCity", ControlKind::ComboBox);
    combo.drop_list = true;
    combo.text_column = 1;
    add(combo);
    let mut toggle = Control::new("tglShow", ControlKind::ToggleButton);
    toggle.caption = "Show".into();
    toggle.enabled = false;
    toggle.visible = false;
    add(toggle);
    let mut spin = Control::new("spnQty", ControlKind::SpinButton);
    spin.min = 1;
    spin.max = 10;
    spin.position = 5;
    spin.small_change = 2;
    spin.orientation = 0;
    add(spin);
    let mut scroll = Control::new("scrZoom", ControlKind::ScrollBar);
    scroll.large_change = 10;
    scroll.orientation = 1;
    add(scroll);
    let mut strip = Control::new("tabView", ControlKind::TabStrip);
    strip.tabs = vec!["One".into(), "Two".into(), "Tiga".into()];
    strip.tab_index_selected = 1;
    add(strip);
    let mut image = Control::new("imgLogo", ControlKind::Image);
    image.picture = Some(Picture::new(vec![
        0x89, b'P', b'N', b'G', 13, 10, 26, 10, 1, 2,
    ]));
    image.picture_size_mode = 3;
    add(image);
    let mut frame = Control::new("fraShip", ControlKind::Frame);
    frame.caption = "Shipping".into();
    frame.left = 18.0;
    frame.top = 126.0;
    frame.width = 144.0;
    frame.height = 90.0;
    let frame = add(frame);
    for (n, name) in ["optAir", "optSea"].into_iter().enumerate() {
        let mut opt = Control::new(name, ControlKind::OptionButton);
        opt.caption = name[3..].into();
        opt.group_name = "Ship".into();
        opt.parent = Some(frame);
        opt.top = 18.0 * n as f32;
        opt.value = if n == 0 { "1".into() } else { "0".into() };
        add(opt);
    }
    let mut pages = Control::new("mpgMore", ControlKind::MultiPage);
    pages.left = 180.0;
    pages.top = 126.0;
    pages.width = 162.0;
    pages.height = 108.0;
    pages.tab_index_selected = 1;
    let mp = add(pages);
    for (n, caption) in ["General", "Notes"].into_iter().enumerate() {
        let mut page = Control::new(&format!("Page{}", n + 1), ControlKind::Page);
        page.caption = caption.into();
        page.parent = Some(mp);
        page.width = 162.0;
        page.height = 90.0;
        let page = add(page);
        let mut note = Control::new(&format!("txtNote{}", n + 1), ControlKind::TextBox);
        note.parent = Some(page);
        note.value = format!("page {}", n + 1);
        add(note);
    }
    f.controls[mp].tabs = vec!["General".into(), "Notes".into()];
    // Controls without text properties take the form's font.
    for c in &mut f.controls {
        if matches!(
            c.kind,
            ControlKind::Image | ControlKind::SpinButton | ControlKind::ScrollBar
        ) {
            c.font = f.font.clone();
        }
    }
    f
}

#[test]
fn written_forms_read_back() {
    let design = every_control();
    let bin = Builder::new(1252)
        .module(
            "Module1",
            ModuleKind::Standard,
            "Sub Go()\nUserForm1.Show\nEnd Sub\n",
        )
        .form(&design, "Private Sub cmdOK_Click()\nHide\nEnd Sub\n")
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(p.module("UserForm1").unwrap().kind, ModuleKind::Form);
    assert!(p.module("UserForm1").unwrap().code.contains("cmdOK_Click"));
    let back = p.form("UserForm1").unwrap().unwrap();
    for (a, b) in design.controls.iter().zip(&back.controls) {
        assert_eq!(a, b, "{}", a.name);
    }
    assert_eq!(back, design);
    let mp = back.control("mpgMore").unwrap();
    assert_eq!(mp.tabs, ["General", "Notes"]);
    assert_eq!(back.control("Page2").unwrap().caption, "Notes");
    let note = back.control("txtNote2").unwrap();
    assert_eq!(back.controls[note.parent.unwrap()].name, "Page2");
    assert_eq!(
        back.control("imgLogo")
            .unwrap()
            .picture
            .as_ref()
            .unwrap()
            .format,
        PictureFormat::Png
    );
}

#[test]
fn a_bare_form_module_gets_an_empty_designer() {
    let bin = Builder::new(1252)
        .module("UserForm1", ModuleKind::Form, "")
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    // The builder writes an empty default design for a bare form module.
    let f = p.form("UserForm1").unwrap().unwrap();
    assert!(f.controls.is_empty());
    assert!(p.form("Module1").is_none());
}

#[test]
fn damaged_control_data_keeps_the_rest_of_the_form() {
    use std::collections::HashMap;
    let design = every_control();
    let mut streams: HashMap<String, Vec<u8>> = kantor_vba::forms::write_streams(&design)
        .into_iter()
        .collect();
    // Cut the object stream short: later controls keep their sites.
    let o = streams.get_mut("o").unwrap();
    o.truncate(o.len() / 3);
    let back = kantor_vba::forms::read_streams("UserForm1", &streams).unwrap();
    assert_eq!(back.controls.len(), design.controls.len());
    assert_eq!(back.control("imgLogo").unwrap().kind, ControlKind::Image);
    assert!(kantor_vba::forms::read_streams("X", &HashMap::new()).is_err());
}

#[test]
fn bare_device_independent_bitmaps_get_a_file_header() {
    let mut dib = vec![0u8; 40 + 8 + 4];
    dib[0] = 40;
    dib[4] = 1; // width
    dib[8] = 1; // height
    dib[12] = 1; // planes
    dib[14] = 1; // bits per pixel: two palette entries
    let p = Picture::new(dib);
    assert_eq!(p.format, PictureFormat::Bmp);
    assert_eq!(&p.bytes[..2], b"BM");
    assert_eq!(
        u32::from_le_bytes(p.bytes[10..14].try_into().unwrap()),
        14 + 40 + 8
    );
}
