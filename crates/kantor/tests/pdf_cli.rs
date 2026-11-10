//! `kantor pdf` end to end: the real binary on PDFs made in the test.

use std::path::Path;
use std::process::{Command, Output};

use kantor_pdf::testing::{sample_jpeg, sample_pdf, sample_pdf_encrypted, sample_png};

fn kantor(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kantor"))
        .arg("pdf")
        .args(args)
        .current_dir(dir)
        .env_remove("KANTOR_PDF_PASSWORD")
        .output()
        .expect("run kantor pdf")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let o = kantor(dir, args);
    assert!(o.status.success(), "{args:?} failed: {}", stderr(&o));
    stdout(&o)
}

/// Each page's width minus 600: which sample page it is.
fn page_order(path: &Path) -> Vec<u32> {
    let info = kantor_pdf::info(&std::fs::read(path).unwrap(), None).unwrap();
    info.pages
        .iter()
        .map(|p| (p.width - 600.0).round() as u32)
        .collect()
}

#[test]
fn help_lists_tools() {
    let dir = tempfile::tempdir().unwrap();
    let out = ok(dir.path(), &["--help"]);
    for tool in [
        "merge",
        "split",
        "rotate",
        "watermark",
        "page-numbers",
        "compress",
        "images",
    ] {
        assert!(out.contains(tool), "{tool} missing from:\n{out}");
    }
    assert!(ok(dir.path(), &["split", "--help"]).contains("--every"));
    let o = kantor(dir.path(), &[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("usage: kantor pdf"));
}

#[test]
fn merge_rotate_and_refuse_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("a.pdf"), sample_pdf(2)).unwrap();
    std::fs::write(d.join("b.pdf"), sample_pdf(4)).unwrap();

    let out = ok(d, &["merge", "a.pdf", "b.pdf:4,1", "-o", "m.pdf"]);
    assert_eq!(out.trim(), "merged 2 files → m.pdf (4 pages)");
    assert_eq!(page_order(&d.join("m.pdf")), [1, 2, 4, 1]);

    // A second run must not clobber the output.
    let o = kantor(d, &["merge", "a.pdf", "-o", "m.pdf"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("already exists (use -f to overwrite)"),
        "{}",
        stderr(&o)
    );
    assert_eq!(page_order(&d.join("m.pdf")).len(), 4);
    ok(d, &["merge", "a.pdf", "-o", "m.pdf", "-f"]);
    assert_eq!(page_order(&d.join("m.pdf")).len(), 2);

    let out = ok(d, &["rotate", "b.pdf", "90", "--pages", "2-3"]);
    assert!(
        out.contains("rotated 2 pages 90° clockwise → b-rotated.pdf"),
        "{out}"
    );
    let info = kantor_pdf::info(&std::fs::read(d.join("b-rotated.pdf")).unwrap(), None).unwrap();
    let rot: Vec<i64> = info.pages.iter().map(|p| p.rotation).collect();
    assert_eq!(rot, [0, 90, 90, 0]);
}

#[test]
fn split_extract_delete_reorder() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("doc.pdf"), sample_pdf(12)).unwrap();

    let out = ok(d, &["split", "doc.pdf", "--every", "5", "-o", "parts"]);
    assert!(out.contains("into 3 files"), "{out}");
    assert_eq!(page_order(&d.join("parts/doc-01-05.pdf")), [1, 2, 3, 4, 5]);
    assert_eq!(page_order(&d.join("parts/doc-11-12.pdf")), [11, 12]);
    // Existing parts block a second split.
    assert_eq!(
        kantor(d, &["split", "doc.pdf", "--every", "5", "-o", "parts"])
            .status
            .code(),
        Some(1)
    );
    ok(d, &["split", "doc.pdf", "--ranges", "3,7-", "-o", "parts"]);
    assert_eq!(page_order(&d.join("parts/doc-03.pdf")), [3]);
    assert_eq!(page_order(&d.join("parts/doc-07-12.pdf")).len(), 6);

    ok(d, &["extract", "doc.pdf", "12,1-2", "-o", "x.pdf"]);
    assert_eq!(page_order(&d.join("x.pdf")), [12, 1, 2]);
    let out = ok(d, &["delete", "doc.pdf", "2-11"]);
    assert!(
        out.contains("deleted 10 pages → doc-deleted.pdf (2 pages)"),
        "{out}"
    );
    assert_eq!(page_order(&d.join("doc-deleted.pdf")), [1, 12]);
    ok(d, &["reorder", "doc-deleted.pdf", "last-1", "-o", "r.pdf"]);
    assert_eq!(page_order(&d.join("r.pdf")), [12, 1]);
}

#[test]
fn bad_ranges_and_missing_files_fail_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("doc.pdf"), sample_pdf(3)).unwrap();
    let o = kantor(d, &["extract", "doc.pdf", "2-9"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("page 9 is out of range (the document has 3 pages)"),
        "{}",
        stderr(&o)
    );
    let o = kantor(d, &["delete", "doc.pdf", "1,,2"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("empty item"));
    let o = kantor(d, &["info", "missing.pdf"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("missing.pdf"));
    let o = kantor(d, &["rotate", "doc.pdf", "45"]);
    assert!(stderr(&o).contains("multiple of 90"));
    assert!(!d.join("doc-rotated.pdf").exists());
}

#[test]
fn watermark_numbers_compress_info() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("doc.pdf"), sample_pdf(3)).unwrap();
    let out = ok(
        d,
        &[
            "watermark",
            "doc.pdf",
            "--text",
            "DRAFT",
            "--opacity",
            "20%",
            "-o",
            "w.pdf",
        ],
    );
    assert!(out.contains("watermarked 3 pages with \"DRAFT\""), "{out}");
    let out = ok(
        d,
        &[
            "page-numbers",
            "w.pdf",
            "--format",
            "{n} of {total}",
            "-o",
            "n.pdf",
        ],
    );
    assert!(out.contains("numbered 3 pages (1–3)"), "{out}");
    let out = ok(d, &["compress", "n.pdf", "-o", "c.pdf"]);
    assert!(out.contains("→ c.pdf"), "{out}");
    let info = ok(d, &["info", "c.pdf"]);
    assert!(info.contains("Pages:      3"), "{info}");
    assert!(info.contains("Title:      Sample"), "{info}");
    let json: serde_json::Value =
        serde_json::from_str(&ok(d, &["info", "c.pdf", "--json"])).unwrap();
    assert_eq!(json["page_count"], 3);
    assert_eq!(json["pages"][0]["width"], 601.0);
}

#[test]
fn images_to_pdf() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("a.png"), sample_png(30, 20, true)).unwrap();
    std::fs::write(d.join("b.jpg"), sample_jpeg(20, 30)).unwrap();
    let out = ok(
        d,
        &[
            "images",
            "a.png",
            "b.jpg",
            "--page-size",
            "letter",
            "--margin",
            "36",
            "-o",
            "i.pdf",
        ],
    );
    assert!(out.starts_with("2 images → i.pdf (2 pages"), "{out}");
    let info = kantor_pdf::info(&std::fs::read(d.join("i.pdf")).unwrap(), None).unwrap();
    assert_eq!(
        info.pages[0].paper_name().as_deref(),
        Some("Letter landscape")
    );
    assert_eq!(info.pages[1].paper_name().as_deref(), Some("Letter"));
    let o = kantor(d, &["images", "a.png"]);
    assert!(stderr(&o).contains("-o OUT.pdf"));
}

#[test]
fn encrypted_input() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("locked.pdf"), sample_pdf_encrypted(2, "o", "pw")).unwrap();
    let o = kantor(d, &["rotate", "locked.pdf", "90"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("encrypted; pass --password"),
        "{}",
        stderr(&o)
    );
    let o = kantor(d, &["rotate", "locked.pdf", "90", "--password", "bad"]);
    assert!(stderr(&o).contains("password does not open"));
    ok(
        d,
        &[
            "rotate",
            "locked.pdf",
            "90",
            "--password",
            "pw",
            "-o",
            "open.pdf",
        ],
    );
    assert_eq!(page_order(&d.join("open.pdf")).len(), 2);
    let info = ok(d, &["info", "locked.pdf"]);
    assert!(info.contains("Encrypted:  yes (pass --password"), "{info}");
}
