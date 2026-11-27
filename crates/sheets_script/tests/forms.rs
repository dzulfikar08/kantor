//! A real Excel-made UserForm (tests/fixtures/README.md) run headless: the
//! answers fill its controls and click its buttons, its handlers write to
//! the sheet.

use sheets_engine::Workbook;
use sheets_engine::vba::ModuleKind;
use sheets_script::run::ModuleSource;
use sheets_script::{Answer, FormAnswer, HeadlessUi, MacroProject, RunOptions, View, run_headless};

const STUDENTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ayliinkaya-StudentRegistration.xlsm"
);

fn cell(book: &Workbook, sheet: u32, a1: &str) -> String {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.display_value(sheet, r, c)
}

/// The workbook's project with a macro that shows the form (the file has
/// none: it showed the form from a button).
fn students() -> (Workbook, MacroProject) {
    let book = Workbook::open(STUDENTS).unwrap();
    let mut macros = MacroProject::of_book(&book).unwrap().unwrap();
    macros.modules.push(ModuleSource::new(
        "Module1",
        ModuleKind::Standard,
        "Sub Go()\r\n    KayitFormu.Show\r\n    Range(\"H1\").Value = \"done\"\r\nEnd Sub\r\n",
    ));
    (book, macros)
}

fn text(s: &str) -> Answer {
    Answer::Text(s.to_owned())
}

#[test]
fn a_real_form_registers_a_student() {
    let (book, macros) = students();
    assert_eq!(macros.forms.len(), 1);
    assert_eq!(macros.forms[0].name, "KayitFormu");
    let ui = HeadlessUi::new(Vec::new(), false).with_form_answers(
        "kayitformu",
        vec![
            // Nothing filled in: the form says so with a MsgBox.
            FormAnswer {
                set: Vec::new(),
                click: Some("CommandButton1".into()),
            },
            FormAnswer {
                set: vec![
                    ("TextBox2".into(), text("Ada")),
                    ("TextBox3".into(), text("Lovelace")),
                    ("TextBox4".into(), text("555")),
                    // ComboBox2_Change copies the lab number to ComboBox3.
                    ("ComboBox2".into(), text("2")),
                ],
                click: Some("CommandButton1".into()),
            },
            // TEMİZLE: clears every TextBox through Me.Controls.
            FormAnswer {
                set: Vec::new(),
                click: Some("CommandButton2".into()),
            },
            // KAPAT: KayitFormu.Hide.
            FormAnswer {
                set: Vec::new(),
                click: Some("CommandButton4".into()),
            },
        ],
    );
    let views = ui.form_views.clone();
    let shown = ui.shown.clone();
    let (book, outcome) = run_headless(
        book,
        &macros,
        "Go",
        View::default(),
        ui,
        &RunOptions::default(),
    );
    if let Err(e) = &outcome.result {
        panic!("{e}");
    }
    assert_eq!(shown.borrow().len(), 1);
    assert!(shown.borrow()[0].starts_with("Tüm Alanları"));
    let views = views.borrow();
    assert_eq!(views.len(), 4);
    let first = &views[0];
    assert_eq!(first.design.caption, "KayitFormu");
    let combo = first.control("ComboBox2").unwrap();
    // RowSource VERİ!LABNO: the header and labs 1 to 5.
    assert_eq!(first.state[combo].list.len(), 6, "{:?}", first.state[combo]);
    assert_eq!(first.state[combo].list[2], ["2"]);
    assert_eq!(first.state[combo].text, "Laboratuvar Seçiniz");
    let after = &views[2];
    let c3 = after.control("ComboBox3").unwrap();
    assert_eq!(after.state[c3].text, "2");
    let last = &views[3];
    let t2 = last.control("TextBox2").unwrap();
    assert_eq!(last.state[t2].text, "");
    // The new row on ANASAYFA (row 5, after three students).
    assert_eq!(cell(&book, 0, "A5"), "4");
    assert_eq!(cell(&book, 0, "B5"), "Ada");
    assert_eq!(cell(&book, 0, "C5"), "Lovelace");
    assert_eq!(cell(&book, 0, "D5"), "555");
    assert_eq!(cell(&book, 0, "E5"), "2");
    assert_eq!(cell(&book, 0, "F5"), "2");
    assert_eq!(cell(&book, 0, "H1"), "done");
}

#[test]
fn a_form_without_answers_is_a_clear_error() {
    let (book, macros) = students();
    let ui = HeadlessUi::new(Vec::new(), false);
    let (book, outcome) = run_headless(
        book,
        &macros,
        "Go",
        View::default(),
        ui,
        &RunOptions::default(),
    );
    let e = outcome.result.unwrap_err();
    assert_eq!(e.number, 445);
    assert!(e.description.contains("KayitFormu"), "{}", e.description);
    assert!(
        e.description.contains("--form-answers"),
        "{}",
        e.description
    );
    assert_eq!(cell(&book, 0, "H1"), "");
}
