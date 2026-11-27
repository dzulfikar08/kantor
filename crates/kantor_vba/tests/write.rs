//! Replacing a module's source: read back by our own reader, compiled
//! caches dropped.

use std::io::{Cursor, Read};

use kantor_vba::synth::Builder;
use kantor_vba::{EMPTY_VBA_PROJECT_STREAM, ModuleKind, VbaProject};

const SIMPLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/poi-SimpleMacro.vbaProject.bin"
));

fn stream(bin: &[u8], path: &str) -> Vec<u8> {
    let mut cfb = cfb::CompoundFile::open(Cursor::new(bin)).unwrap();
    let mut out = Vec::new();
    cfb.open_stream(path)
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn vba_streams(bin: &[u8]) -> Vec<String> {
    let cfb = cfb::CompoundFile::open(Cursor::new(bin)).unwrap();
    cfb.read_storage("/VBA")
        .unwrap()
        .map(|e| e.name().to_owned())
        .collect()
}

/// Caches gone: the 7-byte `_VBA_PROJECT`, no `__SRP_*`, every module
/// stream starting with its compressed source (text offset 0).
fn assert_no_caches(bin: &[u8], modules: &[&str]) {
    assert_eq!(stream(bin, "/VBA/_VBA_PROJECT"), EMPTY_VBA_PROJECT_STREAM);
    let names = vba_streams(bin);
    assert!(!names.iter().any(|n| n.starts_with("__SRP_")), "{names:?}");
    for m in modules {
        let s = stream(bin, &format!("/VBA/{m}"));
        assert_eq!(s.first(), Some(&0x01), "{m}");
        assert!(kantor_vba::compression::decompress(&s).is_ok(), "{m}");
    }
}

fn sample() -> Vec<u8> {
    Builder::new(1252)
        .module(
            "Module1",
            ModuleKind::Standard,
            "Sub Old()\n    MsgBox \"old\"\nEnd Sub\n",
        )
        .module(
            "Helpers",
            ModuleKind::Class,
            "Public Function Add(a, b)\n    Add = a + b\nEnd Function\n",
        )
        .module("Sheet1", ModuleKind::Document, "")
        .module(
            "UserForm1",
            ModuleKind::Form,
            "Private Sub UserForm_Click()\nEnd Sub\n",
        )
        .with_caches()
        .build()
        .unwrap()
}

#[test]
fn replaced_code_reads_back() {
    let bin = sample();
    assert!(vba_streams(&bin).iter().any(|n| n == "__SRP_0"));
    let p = VbaProject::read(&bin).unwrap();
    let new_code = "Sub New1()\r\n    MsgBox \"new\"\r\nEnd Sub\r\n";
    let out = p.with_module_code("module1", new_code).unwrap();
    let q = VbaProject::read(&out).unwrap();
    assert_eq!(q.modules().len(), 4);
    let m = q.module("Module1").unwrap();
    assert_eq!(m.code, "Sub New1()\n    MsgBox \"new\"\nEnd Sub\n");
    assert_eq!(m.attribute("VB_Name").as_deref(), Some("Module1"));
    assert_eq!(m.procedures()[0].name, "New1");
    // The others as they were.
    for name in ["Helpers", "Sheet1", "UserForm1"] {
        let (a, b) = (p.module(name).unwrap(), q.module(name).unwrap());
        assert_eq!(a.source, b.source, "{name}");
        assert_eq!(a.kind, b.kind, "{name}");
    }
    assert_no_caches(&out, &["Module1", "Helpers", "Sheet1", "UserForm1"]);
    // The PROJECT stream and the form's designer are untouched.
    assert_eq!(stream(&out, "/PROJECT"), stream(&bin, "/PROJECT"));
    assert_eq!(
        stream(&out, "/UserForm1/\u{3}VBFrame"),
        stream(&bin, "/UserForm1/\u{3}VBFrame")
    );
    // Twice: a project without caches can be edited again.
    let again = q.with_module_code("Helpers", "' nothing\n").unwrap();
    let r = VbaProject::read(&again).unwrap();
    assert_eq!(r.module("Helpers").unwrap().code, "' nothing\n");
    assert_eq!(r.module("Module1").unwrap().code, m.code);
}

#[test]
fn excel_project_keeps_procedure_attributes() {
    let p = VbaProject::read(SIMPLE).unwrap();
    let out = p
        .with_module_code(
            "Module1",
            "Sub TestMacro()\n    Range(\"B2\").Select\nEnd Sub\n\nSub Another()\nEnd Sub\n",
        )
        .unwrap();
    let q = VbaProject::read(&out).unwrap();
    let m = q.module("Module1").unwrap();
    assert!(
        m.source.starts_with(
            "Attribute VB_Name = \"Module1\"\r\nSub TestMacro()\r\nAttribute TestMacro.VB_Description = \"This is a test macro\"\r\nAttribute TestMacro.VB_ProcData.VB_Invoke_Func = "
        ),
        "{}",
        m.source
    );
    assert_eq!(
        m.code,
        "Sub TestMacro()\n    Range(\"B2\").Select\nEnd Sub\n\nSub Another()\nEnd Sub\n"
    );
    assert_eq!(q.references.len(), p.references.len());
    assert_no_caches(
        &out,
        &["Module1", "ThisWorkbook", "Sheet1", "Sheet2", "Sheet3"],
    );
    for name in ["ThisWorkbook", "Sheet1", "Sheet2", "Sheet3"] {
        assert_eq!(
            p.module(name).unwrap().source,
            q.module(name).unwrap().source
        );
    }
}

#[test]
fn code_with_its_own_attributes_is_written_as_given() {
    let p = VbaProject::read(&sample()).unwrap();
    let code =
        "Attribute VB_Name = \"Module1\"\nAttribute VB_Description = \"mine\"\nSub X()\nEnd Sub\n";
    let q = VbaProject::read(&p.with_module_code("Module1", code).unwrap()).unwrap();
    let m = q.module("Module1").unwrap();
    assert_eq!(m.source, code.replace('\n', "\r\n"));
    assert_eq!(m.attribute("VB_Description").as_deref(), Some("mine"));
}

#[test]
fn code_pages_on_write() {
    let p = VbaProject::read(&sample()).unwrap();
    // Cyrillic doesn't fit code page 1252.
    assert!(p.with_module_code("Module1", "' Привет\n").is_err());
    assert!(p.with_module_code("NoSuchModule", "").is_err());

    let bin = Builder::new(932)
        .module("Module1", ModuleKind::Standard, "' 古い\n")
        .with_caches()
        .build()
        .unwrap();
    let p = VbaProject::read(&bin).unwrap();
    let out = p
        .with_module_code("Module1", "Sub 新しい()\n    MsgBox \"完了\"\nEnd Sub\n")
        .unwrap();
    let q = VbaProject::read(&out).unwrap();
    assert_eq!(
        q.modules()[0].code,
        "Sub 新しい()\n    MsgBox \"完了\"\nEnd Sub\n"
    );
}

#[test]
fn long_replacement_spans_chunks() {
    let p = VbaProject::read(&sample()).unwrap();
    let code: String = (0..2000)
        .map(|i| format!("' line {i}: {}\n", "x".repeat(i % 50)))
        .collect();
    let q = VbaProject::read(&p.with_module_code("Helpers", &code).unwrap()).unwrap();
    assert_eq!(q.module("Helpers").unwrap().code, code);
}
