//! Reading VBA projects: Excel-made ones (Apache POI's test data, see
//! tests/fixtures/README.md) and ones written by `synth::Builder`.

use std::io::{Cursor, Write};

use kantor_vba::synth::Builder;
use kantor_vba::{ModuleKind, ProcedureKind, ReferenceKind, VbaProject};

const SIMPLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/poi-SimpleMacro.vbaProject.bin"
));
const NAMES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/poi-testNames.vbaProject.bin"
));

fn names(p: &VbaProject) -> Vec<(&str, ModuleKind)> {
    p.modules()
        .iter()
        .map(|m| (m.name.as_str(), m.kind))
        .collect()
}

#[test]
fn excel_simple_macro() {
    let p = VbaProject::read(SIMPLE).unwrap();
    assert_eq!(p.name, "VBAProject");
    assert_eq!(p.codepage, 1252);
    assert_eq!(
        names(&p),
        [
            ("Module1", ModuleKind::Standard),
            ("ThisWorkbook", ModuleKind::Document),
            ("Sheet1", ModuleKind::Document),
            ("Sheet2", ModuleKind::Document),
            ("Sheet3", ModuleKind::Document),
        ]
    );
    let refs: Vec<_> = p.references.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(refs, ["stdole", "Office"]);
    assert!(p.references[0].libid.contains("stdole2.tlb"));
    assert_eq!(p.references[0].kind, ReferenceKind::Registered);

    let m = p.module("module1").unwrap();
    assert!(m.error.is_none());
    assert_eq!(m.attribute("VB_Name").as_deref(), Some("Module1"));
    assert!(
        m.attributes
            .iter()
            .any(|a| a == "Attribute TestMacro.VB_Description = \"This is a test macro\""),
        "{:?}",
        m.attributes
    );
    assert!(!m.code.contains("Attribute"), "{}", m.code);
    assert!(
        m.source
            .starts_with("Attribute VB_Name = \"Module1\"\r\nSub TestMacro()\r\n")
    );
    assert!(
        m.code
            .starts_with("Sub TestMacro()\n'\n' TestMacro Macro\n"),
        "{}",
        m.code
    );
    assert!(
        m.code
            .contains("    ActiveCell.FormulaR1C1 = \"This is a macro workbook\"\n")
    );
    let procs = m.procedures();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].name, "TestMacro");
    assert!(procs[0].is_macro());
    assert_eq!(procs[0].line, 0);
    assert_eq!(m.code.lines().nth(procs[0].end_line), Some("End Sub"));

    let wb = p.module("ThisWorkbook").unwrap();
    assert_eq!(wb.code, "");
    assert_eq!(
        wb.attribute("VB_Base").as_deref(),
        Some("0{00020819-0000-0000-C000-000000000046}")
    );
}

#[test]
fn excel_functions_and_control_reference() {
    let p = VbaProject::read(NAMES).unwrap();
    assert_eq!(
        names(&p),
        [
            ("Module1", ModuleKind::Standard),
            ("ThisWorkbook", ModuleKind::Document),
            ("Sheet1", ModuleKind::Document),
        ]
    );
    let procs = p.module("Module1").unwrap().procedures();
    let got: Vec<_> = procs
        .iter()
        .map(|p| (p.name.as_str(), p.kind, p.has_arguments, p.is_macro()))
        .collect();
    assert_eq!(
        got,
        [
            ("Macro1", ProcedureKind::Sub, false, true),
            ("myFunc", ProcedureKind::Function, true, false),
            ("myFunc2", ProcedureKind::Function, true, false),
        ]
    );
    let forms: Vec<_> = p
        .references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Control)
        .collect();
    assert_eq!(forms.len(), 1, "{:?}", p.references);
    assert_eq!(forms[0].name, "MSForms");
    assert!(forms[0].libid.contains("FM20.DLL"), "{}", forms[0].libid);
}

#[test]
fn every_kind_of_module() {
    let bin = Builder::new(1252)
        .name("Budget")
        .module(
            "Module1",
            ModuleKind::Standard,
            "Option Explicit\n\nPublic Sub Refresh()\n    Calculate\nEnd Sub\n\nPrivate Function Twice(x)\n    Twice = x * 2\nEnd Function\n",
        )
        .module(
            "Invoice",
            ModuleKind::Class,
            "Private mTotal As Currency\n\nPublic Property Get Total() As Currency\n    Total = mTotal\nEnd Property\n",
        )
        .module(
            "ThisWorkbook",
            ModuleKind::Document,
            "Private Sub Workbook_Open()\n    MsgBox \"Hello\"\nEnd Sub\n",
        )
        .module(
            "UserForm1",
            ModuleKind::Form,
            "Private Sub CommandButton1_Click()\n    Unload Me\nEnd Sub\n",
        )
        .with_caches()
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(p.name, "Budget");
    assert_eq!(
        names(&p),
        [
            ("Module1", ModuleKind::Standard),
            ("Invoice", ModuleKind::Class),
            ("ThisWorkbook", ModuleKind::Document),
            ("UserForm1", ModuleKind::Form),
        ]
    );
    for m in p.modules() {
        assert!(m.error.is_none(), "{}: {:?}", m.name, m.error);
        assert_eq!(m.attribute("VB_Name").as_deref(), Some(m.name.as_str()));
    }
    let m1 = p.module("Module1").unwrap();
    assert_eq!(
        m1.code,
        "Option Explicit\n\nPublic Sub Refresh()\n    Calculate\nEnd Sub\n\nPrivate Function Twice(x)\n    Twice = x * 2\nEnd Function\n"
    );
    let procs = m1.procedures();
    assert_eq!(procs.len(), 2);
    assert_eq!((procs[0].line, procs[0].end_line), (2, 4));
    assert!(procs[1].private);
    assert_eq!(
        p.module("Invoice").unwrap().procedures()[0].kind,
        ProcedureKind::PropertyGet
    );
    assert!(p.module("UserForm1").unwrap().code.contains("Unload Me"));
    assert_eq!(p.references.len(), 1);
    assert_eq!(p.references[0].name, "stdole");
}

#[test]
fn cyrillic_code_page_1251() {
    let code = "Sub Отчёт()\n    ' Сводка за месяц\n    MsgBox \"Привет, мир\"\nEnd Sub\n";
    let bin = Builder::new(1251)
        .module("Модуль1", ModuleKind::Standard, code)
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(p.codepage, 1251);
    let m = &p.modules()[0];
    assert_eq!(m.name, "Модуль1");
    assert_eq!(m.code, code);
    assert_eq!(m.procedures()[0].name, "Отчёт");
    // Stored as one byte per letter, not UTF-8.
    assert!(m.source.contains("Привет"));
}

#[test]
fn japanese_code_page_932() {
    let code = "Sub 集計()\n    ' 月次の合計\n    MsgBox \"こんにちは\"\nEnd Sub\n";
    let bin = Builder::new(932)
        .module("Module1", ModuleKind::Standard, code)
        .module("Sheet1", ModuleKind::Document, "")
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(p.codepage, 932);
    assert_eq!(p.modules()[0].code, code);
    assert_eq!(p.modules()[0].procedures()[0].name, "集計");
    assert_eq!(p.modules()[1].code, "");
}

#[test]
fn long_module_spans_chunks() {
    let mut code = String::new();
    for i in 0..400 {
        code.push_str(&format!(
            "Sub Step{i}()\n    Range(\"A{i}\").Value = {i} * 2\nEnd Sub\n"
        ));
    }
    assert!(code.len() > 4096 * 4);
    let bin = Builder::new(1252)
        .module("Steps", ModuleKind::Standard, &code)
        .with_caches()
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(p.modules()[0].code, code);
    assert_eq!(p.modules()[0].procedures().len(), 400);
}

#[test]
fn a_module_without_its_stream_is_listed() {
    let bin = Builder::new(1252)
        .module("Module1", ModuleKind::Standard, "Sub A()\nEnd Sub\n")
        .module("Module2", ModuleKind::Standard, "Sub B()\nEnd Sub\n")
        .build()
        .unwrap();
    let mut cfb = cfb::CompoundFile::open(Cursor::new(bin)).unwrap();
    cfb.remove_stream("/VBA/Module2").unwrap();
    cfb.flush().unwrap();
    let bin = cfb.into_inner().into_inner();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(p.modules().len(), 2);
    assert!(p.modules()[0].error.is_none());
    assert!(p.modules()[1].error.is_some());
    assert_eq!(p.modules()[1].code, "");
}

#[test]
fn not_a_project() {
    assert!(VbaProject::read(b"not a compound file").is_err());
    // A compound file without VBA/dir.
    let mut cfb = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    cfb.create_stream("/Other")
        .unwrap()
        .write_all(b"x")
        .unwrap();
    cfb.flush().unwrap();
    let bin = cfb.into_inner().into_inner();
    assert!(VbaProject::read(&bin).is_err());
}
