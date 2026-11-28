//! Equations in Word files: LibreOffice's public math test documents
//! (`tests/corpus/docx/math`, see the corpus README) and documents Kantor
//! writes. Every equation reads, converts to LaTeX that reads back, lays
//! out and draws; unedited equations are saved as they were; new ones
//! reopen as the same equations.

use std::io::Read as _;
use std::path::PathBuf;

use docs_engine::{Document, Position, Selection};
use docs_layout::{Engine, RunSource};

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/math");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "docx"))
        .collect();
    v.sort();
    assert!(v.len() >= 10, "{dir:?}");
    v
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

/// The top-level `m:oMathPara` / `m:oMath` elements of a part, as written.
fn math_fragments(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<m:oMath") {
        let tail = &rest[i..];
        let para = tail.starts_with("<m:oMathPara");
        let close = if para { "</m:oMathPara>" } else { "</m:oMath>" };
        let Some(j) = tail.find(close) else { break };
        out.push(tail[..j + close.len()].to_owned());
        rest = &tail[j + close.len()..];
    }
    out
}

#[test]
fn every_equation_reads_lays_out_and_draws() {
    let mut engine = Engine::new();
    for f in fixtures() {
        let doc = Document::open(&f).unwrap();
        let eqs = doc.equations();
        assert!(!eqs.is_empty(), "{f:?}: no equations");
        for (pos, display, latex) in &eqs {
            kantor_math::latex::parse(latex, *display)
                .unwrap_or_else(|e| panic!("{f:?} {pos:?}: {latex}: {e}"));
        }
        let laid = docs_pdf::lay_out(&doc, &mut engine);
        let math_runs: usize = (0..laid.paged.pages.len())
            .map(|p| {
                laid.paged
                    .glyph_runs(p)
                    .iter()
                    .filter(|r| matches!(r.source, RunSource::Math(_)))
                    .count()
            })
            .sum();
        assert!(math_runs > 0, "{f:?}: no equation drawn");
        let pdf = docs_pdf::export(&doc, &mut engine).unwrap();
        assert!(pdf.starts_with(b"%PDF"), "{f:?}");
    }
}

#[test]
fn unedited_equations_are_saved_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    for f in fixtures() {
        let original = std::fs::read(&f).unwrap();
        let before = math_fragments(&part(&original, "word/document.xml"));
        assert!(!before.is_empty(), "{f:?}");
        let mut doc = Document::open(&f).unwrap();
        let out = dir.path().join(f.file_name().unwrap());
        doc.save_as(&out).unwrap();
        let after = part(&std::fs::read(&out).unwrap(), "word/document.xml");
        for m in &before {
            assert!(after.contains(m.as_str()), "{f:?}: changed on save:\n{m}");
        }
    }
}

#[test]
fn new_equations_reopen_as_written() {
    let samples = [
        (r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}", true),
        (r"\sum_{i=1}^{n} x_i", false),
        (r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", true),
        (r"\int_0^1 f(x)\,dx", false),
        (r"\lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n = e", true),
    ];
    let mut doc = Document::new();
    let mut sel = Selection::caret(Position::new(0, 0));
    let mut want = Vec::new();
    for (src, display) in samples {
        let eq = kantor_math::latex::parse(src, display).unwrap();
        want.push(kantor_math::latex::write(&eq));
        sel = doc.insert_equation(sel, &eq);
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("equations.docx");
    doc.save_as(&out).unwrap();
    let xml = part(&std::fs::read(&out).unwrap(), "word/document.xml");
    assert!(xml.contains("xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\""));
    assert_eq!(math_fragments(&xml).len(), samples.len());
    let back = Document::open(&out).unwrap();
    let got: Vec<String> = back.equations().into_iter().map(|e| e.2).collect();
    assert_eq!(got, want);
}
