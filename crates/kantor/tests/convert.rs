//! `kantor convert` end to end: runs the real binary (no window is opened,
//! so this works headless in CI).

use std::path::Path;
use std::process::{Command, Output};

use docs_engine::{Document, Selection};

fn kantor(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kantor"))
        .current_dir(dir)
        .arg("convert")
        .args(args)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .expect("kantor runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn docx(dir: &Path, name: &str, text: &str) {
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), text, None);
    doc.save_as(&dir.join(name)).unwrap();
}

#[test]
fn docx_to_pdf_and_refusing_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    docx(d, "memo.docx", "Rapat dimulai pukul sembilan.");
    let out = kantor(d, &["memo.docx", "memo.pdf"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pdf = std::fs::read(d.join("memo.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 page"));

    // Exists: refused without -f, the file is left alone.
    std::fs::write(d.join("memo.pdf"), b"keep").unwrap();
    let out = kantor(d, &["memo.docx", "memo.pdf"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("use -f to overwrite"),
        "{}",
        stderr(&out)
    );
    assert_eq!(std::fs::read(d.join("memo.pdf")).unwrap(), b"keep");
    let out = kantor(d, &["-f", "memo.docx", "memo.pdf"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        std::fs::read(d.join("memo.pdf"))
            .unwrap()
            .starts_with(b"%PDF-")
    );
}

#[test]
fn several_inputs_into_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    docx(d, "a.docx", "satu");
    docx(d, "b.docx", "dua");
    std::fs::create_dir(d.join("out")).unwrap();
    let out = kantor(
        d,
        &["-o", "out", "--to", "pdf", "a.docx", "b.docx", "c.docx"],
    );
    // c.docx is missing: the others still convert, the exit status says 1.
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("c.docx: no such file"),
        "{}",
        stderr(&out)
    );
    assert!(d.join("out/a.pdf").is_file() && d.join("out/b.pdf").is_file());
}

#[test]
fn spreadsheets_convert_between_csv_tsv_and_xlsx() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("data.csv"), "Item,Qty\nPompa,3\nKompresor,5\n").unwrap();
    let out = kantor(d, &["data.csv", "data.xlsx"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = kantor(d, &["--sheet", "1", "data.xlsx", "data.tsv"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let tsv = std::fs::read_to_string(d.join("data.tsv")).unwrap();
    assert_eq!(tsv.lines().next(), Some("Item\tQty"));
    assert!(tsv.contains("Kompresor\t5"));

    let out = kantor(d, &["--sheet", "Nope", "data.xlsx", "x.csv"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no sheet \"Nope\""),
        "{}",
        stderr(&out)
    );
}

#[test]
fn usage_errors_exit_2_with_the_supported_list() {
    let dir = tempfile::tempdir().unwrap();
    let out = kantor(dir.path(), &["book.xlsx", "book.pdf"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains(".xlsx to .pdf is not supported yet"), "{err}");
    assert!(err.contains("Supported conversions"), "{err}");

    let out = kantor(dir.path(), &["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("usage: kantor convert"));

    let top = Command::new(env!("CARGO_BIN_EXE_kantor"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&top.stdout).contains("kantor convert"));
}
