//! `kantor sheets macros run` end to end: the real binary on `.xlsm` files
//! made in the test, with Excel-made VBA projects (Apache POI's test files,
//! see crates/kantor_vba/tests/fixtures) and ones written by
//! `kantor_vba::synth`.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sheets_engine::Workbook;
use sheets_engine::vba::ModuleKind;
use sheets_engine::vba::synth::Builder;

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("kantor_vba")
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(p).unwrap()
}

fn kantor(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kantor"))
        .arg("sheets")
        .args(args)
        .current_dir(dir)
        .env_remove("KANTOR_OFFICE_PASSWORD")
        .output()
        .expect("run kantor sheets")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut data = Vec::new();
        e.read_to_end(&mut data).unwrap();
        out.insert(e.name().unwrap().into_owned(), data);
    }
    out
}

fn text(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(parts[name].clone()).unwrap()
}

/// An `.xlsm` with `cells` (`(row, col, input)`) and the VBA project `vba`,
/// stored the way Excel stores one.
fn xlsm(dir: &Path, name: &str, cells: &[(u32, u32, &str)], vba: &[u8]) -> PathBuf {
    let plain = dir.join(format!("{name}.plain.xlsx"));
    let mut wb = Workbook::new_empty().unwrap();
    for (r, c, v) in cells {
        wb.set_input(0, *r, *c, v).unwrap();
    }
    wb.save_as(&plain).unwrap();
    let mut parts = parts(&plain);
    let ct = text(&parts, "[Content_Types].xml")
        .replace(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
        )
        .replace(
            "</Types>",
            "<Default Extension=\"bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/></Types>",
        );
    parts.insert("[Content_Types].xml".into(), ct.into_bytes());
    let rels = text(&parts, "xl/_rels/workbook.xml.rels").replace(
        "</Relationships>",
        "<Relationship Id=\"rId99\" Type=\"http://schemas.microsoft.com/office/2006/relationships/vbaProject\" Target=\"vbaProject.bin\"/></Relationships>",
    );
    parts.insert("xl/_rels/workbook.xml.rels".into(), rels.into_bytes());
    let workbook = text(&parts, "xl/workbook.xml");
    let at = workbook.find("<bookViews>").unwrap();
    let workbook = format!(
        "{}<workbookPr codeName=\"ThisWorkbook\"/>{}",
        &workbook[..at],
        &workbook[at..]
    );
    parts.insert("xl/workbook.xml".into(), workbook.into_bytes());
    parts.insert("xl/vbaProject.bin".into(), vba.to_vec());
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let mut names: Vec<&String> = parts.keys().collect();
    names.sort_by_key(|n| *n != "[Content_Types].xml");
    for n in names {
        zip.start_file(n.as_str(), options).unwrap();
        zip.write_all(&parts[n]).unwrap();
    }
    let path = dir.join(format!("{name}.xlsm"));
    std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    path
}

fn shown(path: &Path, row: u32, col: u32) -> String {
    Workbook::open(path).unwrap().display_value(0, row, col)
}

#[test]
fn excel_simple_macro_runs_headless() {
    let dir = tempfile::tempdir().unwrap();
    let book = xlsm(
        dir.path(),
        "simple",
        &[],
        &fixture("poi-SimpleMacro.vbaProject.bin"),
    );
    let o = kantor(
        dir.path(),
        &[
            "macros",
            "run",
            "simple.xlsm",
            "--macro",
            "TestMacro",
            "--out",
            "out.xlsm",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let out = dir.path().join("out.xlsm");
    assert_eq!(shown(&out, 0, 0), "This is a macro workbook");
    // The macros are kept, the source file is untouched.
    assert!(Workbook::open(&out).unwrap().has_macros());
    assert_eq!(shown(&book, 0, 0), "");
}

#[test]
fn excel_functions_work_in_formulas_when_the_macro_runs() {
    let dir = tempfile::tempdir().unwrap();
    xlsm(
        dir.path(),
        "names",
        &[(0, 1, "=myFunc(\"x\")"), (1, 1, "=myFunc2(\"y\")")],
        &fixture("poi-testNames.vbaProject.bin"),
    );
    // In place: the file itself is saved.
    let o = kantor(
        dir.path(),
        &["macros", "run", "names.xlsm", "--macro", "Module1.Macro1"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let path = dir.path().join("names.xlsm");
    assert_eq!(shown(&path, 0, 0), "w");
    assert_eq!(shown(&path, 0, 1), "xabc");
    assert_eq!(shown(&path, 1, 1), "yabc2");
    // The formulas are saved as written.
    assert_eq!(
        Workbook::open(&path).unwrap().input(0, 0, 1),
        "=myFunc(\"x\")"
    );
}

#[test]
fn dialogs_errors_and_unknown_macros() {
    let dir = tempfile::tempdir().unwrap();
    let code = "Sub Greet()\r\n    Dim n\r\n    n = InputBox(\"How many?\")\r\n    Range(\"A1\").Value = n * 2\r\n    MsgBox \"Doubled\", vbInformation, \"Done\"\r\n    Debug.Print \"printed\"\r\nEnd Sub\r\n\r\nSub Broken()\r\n    Range(\"A1\").Value = 1\r\n    Worksheets(\"Nope\").Activate\r\nEnd Sub\r\n";
    let vba = Builder::new(1252)
        .module("Module1", ModuleKind::Standard, code)
        .build()
        .unwrap();
    xlsm(dir.path(), "greet", &[], &vba);
    let o = kantor(
        dir.path(),
        &[
            "macros",
            "run",
            "greet.xlsm",
            "--macro",
            "Greet",
            "--input",
            "21",
            "-o",
            "g.xlsm",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("[MsgBox] Done: Doubled"), "{err}");
    assert!(err.contains("printed"), "{err}");
    assert_eq!(shown(&dir.path().join("g.xlsm"), 0, 0), "42");

    // No input given: a clear failure, nothing written.
    let o = kantor(
        dir.path(),
        &[
            "macros",
            "run",
            "greet.xlsm",
            "--macro",
            "Greet",
            "-o",
            "h.xlsm",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("--input"), "{}", stderr(&o));
    assert!(!dir.path().join("h.xlsm").exists());

    // A run-time error as Excel words it, with the line.
    let o = kantor(
        dir.path(),
        &[
            "macros",
            "run",
            "greet.xlsm",
            "--macro",
            "Broken",
            "-o",
            "b.xlsm",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.contains("Run-time error '9'"), "{err}");
    assert!(err.contains("Module1.Broken, line 11"), "{err}");
    assert!(!dir.path().join("b.xlsm").exists());

    let o = kantor(
        dir.path(),
        &["macros", "run", "greet.xlsm", "--macro", "Nope"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Module1.Greet"), "{}", stderr(&o));
}

#[test]
fn events_run_workbook_open_first_and_paths_need_allow_path() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let mut data = Workbook::new_empty().unwrap();
    data.set_input(0, 0, 0, "7").unwrap();
    data.save_as(data_dir.join("Data.xlsx")).unwrap();
    let this = "Public Greeting As String\r\n\r\nPrivate Sub Workbook_Open()\r\n    Greeting = \"opened\"\r\nEnd Sub\r\n\r\nPrivate Sub Workbook_BeforeSave(ByVal SaveAsUI As Boolean, Cancel As Boolean)\r\n    Sheet1.Range(\"C1\").Value = \"saving\"\r\nEnd Sub\r\n";
    let code = "Sub Go()\r\n    Range(\"A1\").Value = ThisWorkbook.Greeting\r\n    Dim wb As Workbook\r\n    Set wb = Workbooks.Open(ThisWorkbook.Path & Application.PathSeparator & \"data\" & Application.PathSeparator & \"Data.xlsx\")\r\n    ThisWorkbook.Sheets(1).Range(\"B1\").Value = wb.Sheets(1).Range(\"A1\").Value * 6\r\n    wb.Close False\r\nEnd Sub\r\n";
    let vba = Builder::new(1252)
        .module("ThisWorkbook", ModuleKind::Document, this)
        .module("Sheet1", ModuleKind::Document, "")
        .module("Module1", ModuleKind::Standard, code)
        .build()
        .unwrap();
    xlsm(dir.path(), "ev", &[], &vba);
    // Without --allow-path the other file is refused, nothing is written.
    let o = kantor(
        dir.path(),
        &[
            "macros", "run", "ev.xlsm", "--macro", "Go", "--events", "-o", "x.xlsm",
        ],
    );
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("didn't let"), "{}", stderr(&o));
    assert!(!dir.path().join("x.xlsm").exists());
    let o = kantor(
        dir.path(),
        &[
            "macros",
            "run",
            "ev.xlsm",
            "--macro",
            "Go",
            "--events",
            "--allow-path",
            "data",
            "-o",
            "y.xlsm",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let out = dir.path().join("y.xlsm");
    assert_eq!(shown(&out, 0, 0), "opened");
    assert_eq!(shown(&out, 0, 1), "42");
    assert_eq!(shown(&out, 0, 2), "saving");
}
