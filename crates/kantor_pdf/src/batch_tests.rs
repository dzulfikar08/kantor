//! Batch actions on synthetic PDFs: stamps where they should be (checked
//! with poppler when installed), signed files kept valid, encrypted files
//! kept encrypted, forms filled from tables, plugins, files and folders.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::Pdf;
use crate::annot::{self, Kind};
use crate::batch::*;
use crate::form::{self, FieldValue};
use crate::testing::{sample_form_pdf, sample_pdf, sample_pdf_encrypted, sample_png};

fn qpdf_ok(bytes: &[u8], password: Option<&str>) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("check.pdf");
    std::fs::write(&path, bytes).unwrap();
    let mut cmd = std::process::Command::new("qpdf");
    if let Some(p) = password {
        cmd.arg(format!("--password={p}"));
    }
    match cmd.arg("--check").arg(&path).output() {
        Ok(out) => {
            if !out.status.success() {
                eprintln!("qpdf: {}", String::from_utf8_lossy(&out.stdout));
            }
            out.status.success()
        }
        Err(_) => true,
    }
}

/// Page `page` (1-based) drawn by poppler at 72 dpi, or `None` without it.
fn poppler(bytes: &[u8], page: u32) -> Option<image::RgbImage> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.pdf");
    std::fs::write(&path, bytes).unwrap();
    let out = dir.path().join("out");
    let p = page.to_string();
    let ok = std::process::Command::new("pdftoppm")
        .args(["-r", "72", "-f", &p, "-l", &p, "-png", "-singlefile"])
        .arg(&path)
        .arg(&out)
        .status()
        .ok()?
        .success();
    assert!(ok, "pdftoppm failed");
    Some(image::open(dir.path().join("out.png")).unwrap().to_rgb8())
}

/// pyHanko's verdict (scripts/pdf/signed_fixtures.py); runs only with
/// KANTOR_PYHANKO=1.
fn pyhanko_ok(bytes: &[u8], strict: bool) -> Option<bool> {
    std::env::var_os("KANTOR_PYHANKO")?;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signed.pdf");
    std::fs::write(&path, bytes).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::process::Command::new("uv")
        .args(["run", "--no-project", "--with", "pyhanko", "python"])
        .arg(root.join("scripts/pdf/signed_fixtures.py"))
        .arg("check")
        .arg(&path)
        .arg(root.join("crates/kantor_pdf/tests/fixtures/signed/test-cert.pem"))
        .args(strict.then_some("strict"))
        .output()
        .ok()?;
    eprintln!(
        "pyhanko: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some(out.status.success())
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/signed")
            .join(name),
    )
    .unwrap()
}

/// A PNG: left half red, right half blue (to see which way it is turned).
fn two_tone(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, _| {
        if x < w / 2 {
            image::Rgb([220, 20, 20])
        } else {
            image::Rgb([20, 20, 220])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

struct Setup {
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("stamp.png"), sample_png(80, 40, false)).unwrap();
        std::fs::write(dir.path().join("two.png"), two_tone(80, 40)).unwrap();
        Setup { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn prepare(&self, steps: Vec<Step>) -> Prepared {
        let b = Batch {
            steps,
            ..Batch::default()
        };
        Prepared::new(&b, self.dir.path(), None, &Plugins::default()).unwrap()
    }
}

fn stamp(image: &str, pages: &str, place: Place, width: f32) -> Stamp {
    Stamp {
        image: image.into(),
        pages: pages.into(),
        place,
        width,
        opacity: 1.0,
    }
}

fn done(run: &FileRun) -> (&[u8], bool) {
    match &run.outcome {
        Outcome::Done { bytes, incremental } => (bytes, *incremental),
        o => panic!("not done: {o:?} {:?}", run.notes),
    }
}

fn stamps(bytes: &[u8], page: u32) -> Vec<annot::Annot> {
    let pdf = Pdf::from_bytes(bytes, Some("")).unwrap();
    annot::read_page(&pdf, page)
        .into_iter()
        .filter(|a| matches!(a.kind, Kind::Stamp { .. }))
        .collect()
}

#[test]
fn stamps_go_on_the_chosen_pages_at_the_anchor() {
    let s = Setup::new();
    let place = Place::Anchor {
        anchor: Anchor::BottomRight,
        dx: 20.0,
        dy: 30.0,
    };
    let p = s.prepare(vec![Step::Stamp(stamp(
        "stamp.png",
        "first,last",
        place,
        100.0,
    ))]);
    let run = run_file(&p, "a.pdf", &sample_pdf(3), None);
    let (bytes, incremental) = done(&run);
    assert!(!incremental);
    assert!(qpdf_ok(bytes, None));
    assert_eq!(stamps(bytes, 1).len(), 0);
    let st = stamps(bytes, 2);
    assert_eq!(st.len(), 1);
    // Page 3 is 603 pt wide, 800 high; the stamp is 100 × 50.
    let r = st[0].rect;
    let want = [603.0 - 20.0 - 100.0, 30.0, 603.0 - 20.0, 80.0];
    for k in 0..4 {
        assert!((r[k] - want[k]).abs() < 0.01, "{r:?} vs {want:?}");
    }
    if let Some(img) = poppler(bytes, 3) {
        // Blue at the stamp's middle (y down in the picture).
        let px = img.get_pixel(533, 800 - 55).0;
        assert!(px[2] > 150 && px[0] < 100, "{px:?}");
        assert_eq!(img.get_pixel(300, 400).0, [255, 255, 255]);
    }
    // Ranges past the end skip the step for that file.
    let p = s.prepare(vec![Step::Stamp(stamp("stamp.png", "5-6", place, 0.0))]);
    let run = run_file(&p, "a.pdf", &sample_pdf(3), None);
    assert_eq!(run.outcome, Outcome::Skipped);
    assert!(matches!(run.notes[0].reason, Reason::Pages(_)));
}

#[test]
fn stamps_stand_upright_on_turned_pages() {
    let s = Setup::new();
    let mut pdf = Pdf::from_bytes(&sample_pdf(1), None).unwrap();
    crate::rotate(&mut pdf, &crate::PageRanges::all(), 90).unwrap();
    let turned = pdf.to_bytes().unwrap();
    let place = Place::Anchor {
        anchor: Anchor::TopLeft,
        dx: 10.0,
        dy: 10.0,
    };
    let p = s.prepare(vec![Step::Stamp(stamp("two.png", "all", place, 120.0))]);
    let run = run_file(&p, "t.pdf", &turned, None);
    let (bytes, _) = done(&run);
    assert!(qpdf_ok(bytes, None));
    let Some(img) = poppler(bytes, 1) else {
        return;
    };
    // Shown 800 wide, 601 high; the stamp 120 × 60 at (10, 10) from the top-left.
    assert_eq!(img.width(), 800);
    let left = img.get_pixel(10 + 20, 10 + 30).0;
    let right = img.get_pixel(10 + 100, 10 + 30).0;
    assert!(left[0] > 150 && left[2] < 100, "left is red: {left:?}");
    assert!(right[2] > 150 && right[0] < 100, "right is blue: {right:?}");
}

#[test]
fn example_places_keep_their_nearest_edges() {
    let e = Example {
        x: 400.0,
        y: 700.0,
        width: 150.0,
        height: 60.0,
        page_width: 595.0,
        page_height: 842.0,
    };
    let place = Place::Example { example: e };
    // A4 → Letter: right and bottom distances kept.
    let (x, y) = place_on(&place, 612.0, 792.0, 150.0, 60.0);
    assert!((x - (612.0 - 45.0 - 150.0)).abs() < 0.01);
    assert!((y - (792.0 - 82.0 - 60.0)).abs() < 0.01);
    // Centred stays centred.
    let e = Example {
        x: (595.0 - 100.0) / 2.0,
        ..e
    };
    let (x, _) = place_on(
        &Place::Example {
            example: Example { width: 100.0, ..e },
        },
        612.0,
        792.0,
        100.0,
        40.0,
    );
    assert!((x - 256.0).abs() < 0.01);
    assert_eq!(
        "bottom right".parse::<Anchor>().unwrap(),
        Anchor::BottomRight
    );
    assert!("nowhere".parse::<Anchor>().is_err());
}

#[test]
fn signed_files_keep_their_signatures() {
    let s = Setup::new();
    let place = Place::Anchor {
        anchor: Anchor::TopRight,
        dx: 36.0,
        dy: 36.0,
    };
    let wm = Watermark {
        text: "COPY".into(),
        ..Watermark::default()
    };
    let p = s.prepare(vec![
        Step::Stamp(stamp("stamp.png", "all", place, 90.0)),
        Step::Watermark(wm.clone()),
        Step::Flatten,
    ]);
    let original = fixture("signed-approval.pdf");
    let run = run_file(&p, "signed-approval.pdf", &original, None);
    let (bytes, incremental) = done(&run);
    assert!(incremental);
    assert!(bytes.starts_with(&original), "signed bytes are a prefix");
    assert!(qpdf_ok(bytes, None));
    assert_eq!(stamps(bytes, 0).len(), 1);
    if let Some(ok) = pyhanko_ok(bytes, false) {
        assert!(ok, "pyhanko: the signature is intact after batch stamping");
    }
    let reasons: Vec<_> = run
        .notes
        .iter()
        .map(|n| (n.step, n.reason.clone()))
        .collect();
    assert_eq!(
        reasons,
        [(Some(1), Reason::Signed), (Some(2), Reason::Signed)]
    );
    // Certified, no changes allowed: nothing is written.
    let run = run_file(&p, "p1.pdf", &fixture("signed-certify-p1.pdf"), None);
    assert_eq!(run.outcome, Outcome::Skipped);
    assert_eq!(run.notes[0].reason, Reason::NoChangesAllowed);
    // Certified for form filling: the stamp is refused, the fill goes in.
    let mut fill = Fill::default();
    fill.values.insert("name".into(), "Budi".into());
    let p = s.prepare(vec![
        Step::Stamp(stamp("stamp.png", "all", place, 90.0)),
        Step::Fill(fill),
    ]);
    let original = fixture("signed-certify-p2.pdf");
    let run = run_file(&p, "p2.pdf", &original, None);
    let (bytes, _) = done(&run);
    assert!(bytes.starts_with(&original));
    assert_eq!(run.notes[0].reason, Reason::FormFillOnly);
    let f = form::read_form(&Pdf::from_bytes(bytes, None).unwrap()).unwrap();
    assert_eq!(
        f.field("name").unwrap().value,
        FieldValue::Text("Budi".into())
    );
    if let Some(ok) = pyhanko_ok(bytes, true) {
        assert!(ok, "pyhanko: batch form fill allowed by DocMDP 2");
    }
}

#[test]
fn encrypted_files_stay_encrypted() {
    let s = Setup::new();
    let p = s.prepare(vec![Step::Stamp(stamp(
        "stamp.png",
        "all",
        Place::Point { x: 50.0, y: 50.0 },
        60.0,
    ))]);
    // Opens without a password (owner password only).
    let run = run_file(&p, "e.pdf", &sample_pdf_encrypted(2, "owner", ""), None);
    let (bytes, _) = done(&run);
    assert!(matches!(
        Pdf::from_bytes(bytes, None),
        Err(crate::Error::Encrypted)
    ));
    assert_eq!(stamps(bytes, 1).len(), 1);
    assert!(qpdf_ok(bytes, None));
    // A user password: skipped without it, done (and still protected) with it.
    let locked = sample_pdf_encrypted(1, "owner", "secret");
    let run = run_file(&p, "l.pdf", &locked, None);
    assert_eq!(run.outcome, Outcome::Skipped);
    assert_eq!(run.notes[0].reason, Reason::Password);
    let run = run_file(&p, "l.pdf", &locked, Some("secret"));
    let (bytes, _) = done(&run);
    assert!(matches!(
        Pdf::from_bytes(bytes, Some("")),
        Err(crate::Error::WrongPassword)
    ));
    assert!(Pdf::from_bytes(bytes, Some("secret")).is_ok());
    assert!(qpdf_ok(bytes, Some("secret")));
}

#[test]
fn forms_fill_from_a_table_row_per_file() {
    let s = Setup::new();
    std::fs::write(
        s.path("values.csv"),
        "file,name,agree,size,city,fruit,nope\n\
         alpha.pdf,Budi,ya,L,bandung,pear,x\n\
         beta,Sari,no,S,Jakarta,apple,\n",
    )
    .unwrap();
    let p = s.prepare(vec![Step::Fill(Fill {
        data: Some("values.csv".into()),
        ..Fill::default()
    })]);
    let run = run_file(&p, "alpha.pdf", &sample_form_pdf(), None);
    let (bytes, _) = done(&run);
    assert!(qpdf_ok(bytes, None));
    let f = form::read_form(&Pdf::from_bytes(bytes, None).unwrap()).unwrap();
    assert_eq!(
        f.field("name").unwrap().value,
        FieldValue::Text("Budi".into())
    );
    assert_eq!(
        f.field("agree").unwrap().value,
        FieldValue::State("Yes".into())
    );
    assert_eq!(
        f.field("size").unwrap().value,
        FieldValue::State("L".into())
    );
    assert_eq!(
        f.field("city").unwrap().value,
        FieldValue::Choice(vec!["Bandung".into()])
    );
    assert_eq!(
        run.notes[0].reason,
        Reason::UnknownFields(vec!["nope".into()])
    );
    let run = run_file(&p, "Beta.PDF", &sample_form_pdf(), None);
    let (bytes, _) = done(&run);
    let f = form::read_form(&Pdf::from_bytes(bytes, None).unwrap()).unwrap();
    assert_eq!(
        f.field("agree").unwrap().value,
        FieldValue::State("Off".into())
    );
    assert_eq!(
        f.field("size").unwrap().value,
        FieldValue::State("S".into())
    );
    // No row, no form.
    let run = run_file(&p, "gamma.pdf", &sample_form_pdf(), None);
    assert_eq!(run.outcome, Outcome::Skipped);
    assert_eq!(run.notes[0].reason, Reason::NoRow);
    let run = run_file(&p, "alpha.pdf", &sample_pdf(1), None);
    assert_eq!(run.notes[0].reason, Reason::NoForm);
}

/// Passwords per file from a table's password column (used to open the
/// file, never filled into the form), or from a passwords table for the
/// run; the owner's restrictions hold for the user password.
#[test]
fn passwords_come_from_the_table() {
    use crate::testing::{Protect, sample_pdf_protected};
    use lopdf::Permissions as P;
    let s = Setup::new();
    std::fs::write(
        s.path("values.csv"),
        "file,name,Password\nalpha.pdf,Budi,alpha-pw\nbeta.pdf,Sari,\n",
    )
    .unwrap();
    let p = s.prepare(vec![Step::Fill(Fill {
        data: Some("values.csv".into()),
        ..Fill::default()
    })]);
    let mut form = Pdf::from_bytes(&sample_form_pdf(), None).unwrap();
    form.doc.trailer.set(
        "ID",
        vec![
            lopdf::Object::String(b"form-id".to_vec(), lopdf::StringFormat::Hexadecimal),
            lopdf::Object::String(b"form-id".to_vec(), lopdf::StringFormat::Hexadecimal),
        ],
    );
    let encrypt = |pdf: &Pdf, user: &str, perms: P| {
        let mut doc = pdf.doc.clone();
        let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password: user,
            key_length: 128,
            permissions: perms,
        })
        .unwrap();
        doc.encrypt(&state).unwrap();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    };
    let alpha = encrypt(&form, "alpha-pw", P::all());
    assert_eq!(p.password_for("Alpha.pdf"), Some("alpha-pw"));
    let run = run_file(&p, "alpha.pdf", &alpha, None);
    let (bytes, _) = done(&run);
    assert!(
        run.notes.is_empty(),
        "the password column isn't a field: {:?}",
        run.notes
    );
    let filled = Pdf::from_bytes(bytes, Some("alpha-pw")).unwrap();
    assert_eq!(
        form::read_form(&filled)
            .unwrap()
            .field("name")
            .unwrap()
            .value,
        FieldValue::Text("Budi".into())
    );
    assert!(qpdf_ok(bytes, Some("alpha-pw")));
    // An empty cell: the run's password.
    let beta = encrypt(&form, "run-pw", P::all());
    assert_eq!(
        run_file(&p, "beta.pdf", &beta, None).outcome,
        Outcome::Skipped
    );
    let run = run_file(&p, "beta.pdf", &beta, Some("run-pw"));
    done(&run);
    // Only the user password and no form filling allowed: skipped.
    let ro = encrypt(&form, "run-pw", P::PRINTABLE);
    let run = run_file(&p, "beta.pdf", &ro, Some("run-pw"));
    assert_eq!(run.outcome, Outcome::Skipped);
    assert_eq!(run.notes[0].reason, Reason::Restricted);
    let run = run_file(&p, "beta.pdf", &ro, Some("owner"));
    done(&run);

    // A passwords table for a batch without one, and stamps restricted.
    let mut p = s.prepare(vec![Step::Stamp(stamp(
        "stamp.png",
        "all",
        Place::Point { x: 50.0, y: 50.0 },
        60.0,
    ))]);
    p.set_passwords(crate::table::Table::from_csv("pdf,kata sandi\nc.pdf,c-pw\n", None).unwrap());
    let c = sample_pdf_protected(1, Protect::Aes256, "owner", "c-pw", P::all());
    let run = run_file(&p, "c.pdf", &c, None);
    let (bytes, _) = done(&run);
    assert_eq!(stamps_pw(bytes, 1, "c-pw"), 1);
    assert!(qpdf_ok(bytes, Some("c-pw")));
    let c = sample_pdf_protected(1, Protect::Aes128, "owner", "c-pw", P::FILLABLE);
    let run = run_file(&p, "c.pdf", &c, None);
    assert_eq!(run.notes[0].reason, Reason::Restricted);
}

fn stamps_pw(bytes: &[u8], page: u32, pw: &str) -> usize {
    let pdf = Pdf::from_bytes(bytes, Some(pw)).unwrap();
    annot::read_page(&pdf, page - 1)
        .iter()
        .filter(|a| matches!(a.kind, Kind::Stamp { .. }))
        .count()
}

#[test]
fn flatten_draws_every_annotation_with_its_popup() {
    let s = Setup::new();
    // A form with a stamp and a note added.
    let mut pdf = Pdf::from_bytes(&sample_form_pdf(), None).unwrap();
    let mut marks = annot::read_page(&pdf, 0);
    let widgets = marks.len();
    marks.push(annot::Annot::new(
        Kind::Note,
        [400.0, 700.0, 420.0, 720.0],
        [1.0, 0.8, 0.2],
    ));
    marks.push(annot::Annot::new(
        Kind::Square,
        [400.0, 600.0, 500.0, 650.0],
        [1.0, 0.0, 0.0],
    ));
    annot::apply_page(&mut pdf, 0, &marks).unwrap();
    let bytes = pdf.to_bytes().unwrap();
    let p = s.prepare(vec![Step::Flatten]);
    let run = run_file(&p, "f.pdf", &bytes, None);
    let (out, _) = done(&run);
    assert!(qpdf_ok(out, None));
    let pdf = Pdf::from_bytes(out, None).unwrap();
    // Only the form's widgets are left: the note's pop-up went with it.
    let raw = annot::page_annots(&pdf.doc, crate::pages::page_ids(&pdf.doc)[0]);
    assert_eq!(raw.len(), widgets, "{raw:?}");
    if let Some(img) = poppler(out, 1) {
        let px = img.get_pixel(400, 800 - 625).0;
        assert!(px[0] > 200 && px[1] < 80, "the square's border: {px:?}");
    }
}

#[test]
fn batch_files_round_trip_and_plugins_run_last() {
    let b = Batch {
        steps: vec![
            Step::Stamp(stamp(
                "my-stamps/logo.png",
                "last",
                Place::Point { x: 1.0, y: 2.0 },
                0.0,
            )),
            Step::Signature(Stamp::default()),
            Step::Fill(Fill::default()),
            Step::PageNumbers(PageNumbers::default()),
            Step::Watermark(Watermark::default()),
            Step::Flatten,
            Step::Plugin {
                kind: "sign".into(),
                options: serde_json::json!({"certificate": "me.p12"}),
            },
        ],
        output: Output {
            suffix: "-signed".into(),
            folder: Some("/out".into()),
        },
        ..Batch::default()
    };
    let json = b.to_json();
    assert!(json.contains("\"type\": \"page-numbers\""), "{json}");
    assert_eq!(Batch::from_json(&json).unwrap(), b);
    assert!(Batch::from_json(r#"{"steps": [{"pages": "1"}]}"#).is_err());
    assert!(Batch::from_json(r#"{"version": 99, "steps": []}"#).is_err());

    struct Tag;
    impl Plugin for Tag {
        fn kind(&self) -> &str {
            "tag"
        }
        fn apply(
            &self,
            options: &serde_json::Value,
            file: &FileContext,
            bytes: &[u8],
        ) -> Result<Applied, String> {
            if file.name == "skip.pdf" {
                return Ok(Applied::Skipped("skipped: not this one".into()));
            }
            let mut v = bytes.to_vec();
            v.extend_from_slice(options["text"].as_str().unwrap_or("").as_bytes());
            Ok(Applied::Changed(v))
        }
    }
    let s = Setup::new();
    let mut plugins = Plugins::default();
    plugins.add(Arc::new(Tag));
    let tag = Step::Plugin {
        kind: "tag".into(),
        options: serde_json::json!({"text": "%TAGGED"}),
    };
    let numbers = Step::PageNumbers(PageNumbers::default());
    let batch = |steps| Batch {
        steps,
        ..Batch::default()
    };
    // Unknown steps and plugins before built-in steps are refused.
    assert!(
        Prepared::new(
            &batch(vec![tag.clone()]),
            s.dir.path(),
            None,
            &Plugins::default()
        )
        .is_err()
    );
    assert!(
        Prepared::new(
            &batch(vec![tag.clone(), numbers.clone()]),
            s.dir.path(),
            None,
            &plugins
        )
        .is_err()
    );
    let p = Prepared::new(&batch(vec![numbers, tag]), s.dir.path(), None, &plugins).unwrap();
    let run = run_file(&p, "a.pdf", &sample_pdf(2), None);
    let (bytes, incremental) = done(&run);
    assert!(bytes.ends_with(b"%TAGGED"));
    // An unsigned file had no signatures to keep, whatever the plugin did.
    assert!(!incremental);
    let run = run_file(&p, "skip.pdf", &sample_pdf(2), None);
    let (bytes, _) = done(&run);
    assert!(!bytes.ends_with(b"%TAGGED"));
    assert_eq!(run.notes.len(), 1);
}

#[test]
fn runs_files_into_a_folder_and_skips_existing_results() {
    let s = Setup::new();
    let src = s.path("in");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.pdf"), sample_pdf(1)).unwrap();
    std::fs::write(src.join("B.PDF"), sample_pdf(2)).unwrap();
    std::fs::write(src.join("bad.pdf"), b"not a pdf").unwrap();
    std::fs::write(src.join("note.txt"), b"x").unwrap();
    std::fs::write(src.join("sub/a.pdf"), sample_pdf(1)).unwrap();
    let mut files = pdfs_in(&src);
    assert_eq!(files.len(), 3);
    files.push(src.join("sub/a.pdf"));
    let p = s.prepare(vec![Step::PageNumbers(PageNumbers::default())]);
    let out = Output {
        suffix: String::new(),
        folder: Some(s.path("out")),
    };
    assert_eq!(
        out.paths_for(&files)[3],
        s.path("out").join("a (2).pdf"),
        "same names in one folder"
    );
    let cancel = AtomicBool::new(false);
    let mut seen = 0;
    let results = run(&p, &files, &out, Existing::Skip, None, &cancel, |_, _| {
        seen += 1
    });
    assert_eq!(seen, 4);
    assert!(matches!(results[0].status, Status::Done { .. }));
    assert!(
        matches!(results[2].status, Status::Failed(_)),
        "{results:?}"
    );
    assert!(s.path("out/B.pdf").exists());
    assert!(results[0].describe(&p.batch.steps).contains("done →"));
    // Again: the results exist and aren't replaced.
    assert_eq!(existing_outputs(&out, &files).len(), 3);
    let results = run(&p, &files, &out, Existing::Skip, None, &cancel, |_, _| {});
    assert_eq!(results[0].notes[0].reason, Reason::Exists);
    // Cancelled before the first file: nothing runs.
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        run(
            &p,
            &files,
            &out,
            Existing::Replace,
            None,
            &cancel,
            |_, _| {}
        )
        .is_empty()
    );
}

#[test]
fn paths_used_names_pictures_tables_and_result_folders() {
    let mut b = Batch::default();
    b.steps.push(Step::Stamp(stamp("seal.png", "all", Place::Center, 0.0)));
    b.steps
        .push(Step::Signature(stamp("my-stamps/Me.ink", "all", Place::Center, 0.0)));
    b.steps.push(Step::Fill(Fill {
        data: Some("/data/rows.csv".into()),
        ..Fill::default()
    }));
    let base = Path::new("/work");
    let inputs = [PathBuf::from("/in/a.pdf"), PathBuf::from("/in/b.pdf"), PathBuf::from("/x/c.pdf")];
    let (files, folders) = b.paths_used(base, &inputs);
    assert_eq!(files, [PathBuf::from("/work/seal.png"), PathBuf::from("/data/rows.csv")]);
    assert_eq!(folders, [PathBuf::from("/in"), PathBuf::from("/x")]);
    b.output.folder = Some("/out".into());
    assert_eq!(b.paths_used(base, &inputs).1, [PathBuf::from("/out")]);
}
