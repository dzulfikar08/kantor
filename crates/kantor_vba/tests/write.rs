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

// ---- with_modules: add, edit, remove, rename ----

use kantor_vba::{ModuleSpec, new_project};

fn project_text(bin: &[u8]) -> String {
    String::from_utf8_lossy(&stream(bin, "/PROJECT")).into_owned()
}

/// Excel's own project: a module and a class added, Module1 edited, then
/// one removed again; everything reads back and the PROJECT stream lists
/// what's there.
#[test]
fn modules_added_edited_removed_in_an_excel_project() {
    let p = VbaProject::read(SIMPLE).unwrap();
    let mut specs: Vec<ModuleSpec> = p
        .modules()
        .iter()
        .map(|m| ModuleSpec::new(&m.name, m.kind, &m.code))
        .collect();
    specs[0].code = "Sub TestMacro()\n    MsgBox \"edited\"\nEnd Sub\n".into();
    specs.push(ModuleSpec::new(
        "Module2",
        ModuleKind::Standard,
        "Sub Hello()\r\n    Range(\"A1\").Value = \"Hi\"\r\nEnd Sub\r\n",
    ));
    specs.push(ModuleSpec::new(
        "Counter",
        ModuleKind::Class,
        "Public N As Long\nPublic Sub Bump()\n    N = N + 1\nEnd Sub\n",
    ));
    let out = p.with_modules(&specs).unwrap();
    let q = VbaProject::read(&out).unwrap();
    let mut expected: Vec<&str> = names_in(&p);
    expected.extend(["Module2", "Counter"]);
    assert_eq!(names_in(&q), expected);
    let m1 = q.module("Module1").unwrap();
    assert_eq!(m1.code, specs[0].code);
    // Its procedure attributes are kept.
    assert!(
        m1.source.contains("TestMacro.VB_Description"),
        "{}",
        m1.source
    );
    let m2 = q.module("Module2").unwrap();
    assert_eq!(m2.kind, ModuleKind::Standard);
    assert_eq!(
        m2.code,
        "Sub Hello()\n    Range(\"A1\").Value = \"Hi\"\nEnd Sub\n"
    );
    assert_eq!(m2.attribute("VB_Name").as_deref(), Some("Module2"));
    let c = q.module("Counter").unwrap();
    assert_eq!(c.kind, ModuleKind::Class);
    assert_eq!(c.procedures()[0].name, "Bump");
    for name in ["ThisWorkbook", "Sheet1", "Sheet2", "Sheet3"] {
        assert_eq!(
            p.module(name).unwrap().source,
            q.module(name).unwrap().source
        );
        assert_eq!(q.module(name).unwrap().kind, ModuleKind::Document);
    }
    assert_eq!(q.references.len(), p.references.len());
    assert_no_caches(
        &out,
        &["Module1", "Module2", "Counter", "ThisWorkbook", "Sheet1"],
    );
    let text = project_text(&out);
    assert!(text.contains("Module=Module2\r\n"), "{text}");
    assert!(text.contains("Class=Counter\r\n"), "{text}");
    assert!(text.contains("Module2=0, 0, 0, 0, C"), "{text}");
    // The protection lines Excel wrote are kept.
    assert!(text.contains("CMG=") && text.contains("DPB=") && text.contains("GC="));

    // Remove Module2 and rename Counter (remove + add with its code).
    let specs2: Vec<ModuleSpec> = q
        .modules()
        .iter()
        .filter(|m| m.name != "Module2")
        .map(|m| {
            if m.name == "Counter" {
                ModuleSpec::new("Tally", m.kind, &m.code)
            } else {
                ModuleSpec::new(&m.name, m.kind, &m.code)
            }
        })
        .collect();
    let out2 = q.with_modules(&specs2).unwrap();
    let r = VbaProject::read(&out2).unwrap();
    assert!(r.module("Module2").is_none());
    assert!(r.module("Counter").is_none());
    assert_eq!(r.module("Tally").unwrap().kind, ModuleKind::Class);
    assert_eq!(r.module("Tally").unwrap().code, c.code);
    assert!(
        !vba_streams(&out2)
            .iter()
            .any(|n| n == "Module2" || n == "Counter")
    );
    let text = project_text(&out2);
    assert!(!text.contains("Module2"), "{text}");
    assert!(!text.contains("Counter"), "{text}");
    assert!(text.contains("Class=Tally\r\n"), "{text}");
}

fn names_in(p: &VbaProject) -> Vec<&str> {
    p.modules().iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn every_module_parses_after_a_rewrite() {
    // Both POI fixtures, with a new module: the interpreter's parser reads
    // every module's code.
    for bin in [
        SIMPLE,
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/poi-testNames.vbaProject.bin"
        ))[..],
    ] {
        let p = VbaProject::read(bin).unwrap();
        let mut specs: Vec<ModuleSpec> = p
            .modules()
            .iter()
            .map(|m| ModuleSpec::new(&m.name, m.kind, &m.code))
            .collect();
        specs.push(ModuleSpec::new(
            "Added",
            ModuleKind::Standard,
            "Option Explicit\n\nSub Added()\n    Dim i As Long\n    For i = 1 To 3\n        Cells(i, 1).Value = i\n    Next i\nEnd Sub\n",
        ));
        let q = VbaProject::read(&p.with_modules(&specs).unwrap()).unwrap();
        assert_eq!(q.modules().len(), specs.len());
        for m in q.modules() {
            assert!(m.error.is_none(), "{}: {:?}", m.name, m.error);
            let spec = specs.iter().find(|s| s.name == m.name).unwrap();
            assert_eq!(
                m.code.trim_end(),
                spec.code.replace("\r\n", "\n").trim_end(),
                "{}",
                m.name
            );
        }
    }
}

#[test]
fn a_new_project_from_scratch() {
    let specs = [
        ModuleSpec::new("ThisWorkbook", ModuleKind::Document, ""),
        ModuleSpec::new("Sheet1", ModuleKind::Document, ""),
        ModuleSpec::new(
            "Module1",
            ModuleKind::Standard,
            "Sub Basics()\n    MsgBox \"Hi\"\nEnd Sub\n",
        ),
    ];
    let bin = new_project(&specs).unwrap();
    let p = VbaProject::read(&bin).unwrap();
    assert_eq!(names_in(&p), ["ThisWorkbook", "Sheet1", "Module1"]);
    assert_eq!(
        p.module("ThisWorkbook")
            .unwrap()
            .attribute("VB_Base")
            .as_deref(),
        Some("0{00020819-0000-0000-C000-000000000046}")
    );
    assert_eq!(
        p.module("Sheet1").unwrap().attribute("VB_Base").as_deref(),
        Some("0{00020820-0000-0000-C000-000000000046}")
    );
    assert_eq!(p.module("Module1").unwrap().procedures()[0].name, "Basics");
    // The protection lines the PROJECT grammar requires, decrypting to
    // "not protected", "no password", "visible".
    let text = project_text(&bin);
    let value = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap()
            .trim_matches('"')
            .to_owned()
    };
    let id = value("ID");
    for (key, data) in [("CMG", vec![0u8; 4]), ("DPB", vec![0]), ("GC", vec![0xFF])] {
        let (version, proj_key, back) = kantor_vba::crypt::decrypt(&value(key)).unwrap();
        assert_eq!(version, 2, "{key}");
        assert_eq!(proj_key, kantor_vba::crypt::project_key(&id), "{key}");
        assert_eq!(back, data, "{key}");
    }
    // Excel's own: our decryption reads them the same way.
    let excel = project_text(SIMPLE);
    let id = excel
        .lines()
        .find_map(|l| l.strip_prefix("ID="))
        .unwrap()
        .trim_matches('"')
        .to_owned();
    let cmg = excel.lines().find_map(|l| l.strip_prefix("CMG=")).unwrap();
    let (version, proj_key, data) = kantor_vba::crypt::decrypt(cmg).unwrap();
    assert_eq!(version, 2);
    assert_eq!(proj_key, kantor_vba::crypt::project_key(&id));
    assert_eq!(data.len(), 4);

    // Bad names and duplicates are refused.
    assert!(new_project(&[ModuleSpec::new("1st", ModuleKind::Standard, "")]).is_err());
    assert!(
        new_project(&[
            ModuleSpec::new("A", ModuleKind::Standard, ""),
            ModuleSpec::new("a", ModuleKind::Class, ""),
        ])
        .is_err()
    );
    let p = VbaProject::read(&bin).unwrap();
    assert!(
        p.with_modules(&[ModuleSpec::new("UserForm9", ModuleKind::Form, "")])
            .is_err()
    );
}

#[test]
fn removing_a_form_removes_its_designer() {
    let bin = sample();
    let p = VbaProject::read(&bin).unwrap();
    let specs: Vec<ModuleSpec> = p
        .modules()
        .iter()
        .filter(|m| m.kind != ModuleKind::Form)
        .map(|m| ModuleSpec::new(&m.name, m.kind, &m.code))
        .collect();
    let out = p.with_modules(&specs).unwrap();
    let cfb = cfb::CompoundFile::open(Cursor::new(&out)).unwrap();
    assert!(!cfb.is_storage("/UserForm1"));
    let q = VbaProject::read(&out).unwrap();
    assert!(q.module("UserForm1").is_none());
    assert!(!project_text(&out).contains("UserForm1"));
}
