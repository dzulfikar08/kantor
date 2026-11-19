//! Annotations and forms: written, read back, checked with qpdf and drawn
//! with poppler (`pdftoppm`) when they are installed.

use std::collections::HashMap;
use std::sync::Arc;

use crate::Pdf;
use crate::annot::{self, Annot, Art, Font, Kind};
use crate::form::{self, FieldKind, FieldValue};
use crate::testing::{sample_form_pdf, sample_pdf, sample_round_stamp};

fn qpdf_ok(bytes: &[u8]) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("check.pdf");
    std::fs::write(&path, bytes).unwrap();
    match std::process::Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
    {
        Ok(out) => {
            if !out.status.success() {
                eprintln!("qpdf: {}", String::from_utf8_lossy(&out.stdout));
            }
            out.status.success()
        }
        Err(_) => true,
    }
}

/// Page 1 drawn by poppler at 72 dpi (1 px = 1 pt), or `None` without it.
fn poppler(bytes: &[u8]) -> Option<image::RgbImage> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.pdf");
    std::fs::write(&path, bytes).unwrap();
    let out = dir.path().join("out");
    let ok = std::process::Command::new("pdftoppm")
        .args(["-r", "72", "-f", "1", "-l", "1", "-png", "-singlefile"])
        .arg(&path)
        .arg(&out)
        .status()
        .ok()?
        .success();
    assert!(ok, "pdftoppm failed");
    Some(image::open(dir.path().join("out.png")).unwrap().to_rgb8())
}

/// The colour at user-space point (x, y) of a page `h` points high.
fn at(img: &image::RgbImage, x: f32, y: f32, h: f32) -> [u8; 3] {
    img.get_pixel(x as u32, (h - y) as u32).0
}

fn save(mut pdf: Pdf) -> Vec<u8> {
    pdf.to_bytes().unwrap()
}

/// KANTOR_PDF_DUMP=dir keeps a test's output for other viewers.
fn dump(name: &str, bytes: &[u8]) {
    if let Some(dir) = std::env::var_os("KANTOR_PDF_DUMP") {
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }
}

fn every_kind() -> Vec<Annot> {
    let mut v = Vec::new();
    let mut hl = Annot::new(
        Kind::Highlight(vec![[
            100.0, 720.0, 200.0, 720.0, 100.0, 700.0, 200.0, 700.0,
        ]]),
        [100.0, 700.0, 200.0, 720.0],
        [1.0, 0.9, 0.0],
    );
    hl.author = "Dewi".into();
    hl.contents = "Check this".into();
    v.push(hl);
    v.push(Annot::new(
        Kind::Underline(vec![[
            100.0, 690.0, 200.0, 690.0, 100.0, 676.0, 200.0, 676.0,
        ]]),
        [100.0, 676.0, 200.0, 690.0],
        [0.0, 0.5, 0.0],
    ));
    v.push(Annot::new(
        Kind::StrikeOut(vec![[
            100.0, 670.0, 200.0, 670.0, 100.0, 656.0, 200.0, 656.0,
        ]]),
        [100.0, 656.0, 200.0, 670.0],
        [1.0, 0.0, 0.0],
    ));
    let mut note = Annot::new(Kind::Note, [400.0, 700.0, 420.0, 720.0], [1.0, 0.85, 0.2]);
    note.contents = "Résumé — überall ✓".into();
    let note_id = note.id.clone();
    v.push(note);
    let mut reply = Annot::new(Kind::Note, [400.0, 700.0, 420.0, 720.0], [1.0, 0.85, 0.2]);
    reply.contents = "Agreed".into();
    reply.reply_to = Some(note_id.clone());
    v.push(reply);
    let mut state = Annot::new(
        Kind::State("Completed".into()),
        [400.0, 700.0, 420.0, 720.0],
        [0.0; 3],
    );
    state.reply_to = Some(note_id);
    v.push(state);
    let mut ft = Annot::new(
        Kind::FreeText {
            font: Font::Courier,
            size: 14.0,
        },
        [100.0, 500.0, 300.0, 560.0],
        [0.0, 0.0, 0.8],
    );
    ft.contents = "A text box that wraps onto more lines".into();
    ft.fill = Some([0.9, 0.95, 1.0]);
    ft.width = 1.5;
    v.push(ft);
    let mut ink = Annot::new(
        Kind::Ink(vec![
            vec![[100.0, 400.0], [150.0, 450.0], [200.0, 400.0]],
            vec![[110.0, 410.0], [190.0, 410.0]],
        ]),
        [0.0; 4],
        [0.1, 0.1, 0.6],
    );
    ink.width = 3.0;
    ink.rect = annot::fit_rect(&ink.kind, 3.0).unwrap();
    v.push(ink);
    let mut sq = Annot::new(Kind::Square, [300.0, 300.0, 400.0, 360.0], [1.0, 0.0, 0.0]);
    sq.width = 2.0;
    sq.fill = Some([1.0, 1.0, 0.8]);
    v.push(sq);
    v.push(Annot::new(
        Kind::Circle,
        [420.0, 300.0, 520.0, 360.0],
        [0.0, 0.0, 1.0],
    ));
    let line = Kind::Line {
        a: [100.0, 250.0],
        b: [250.0, 280.0],
        arrow: true,
    };
    let r = annot::fit_rect(&line, 1.0).unwrap();
    v.push(Annot::new(line, r, [0.0, 0.0, 0.0]));
    let poly = Kind::Polygon(vec![[300.0, 200.0], [380.0, 260.0], [460.0, 200.0]]);
    let r = annot::fit_rect(&poly, 1.0).unwrap();
    v.push(Annot::new(poly, r, [0.5, 0.0, 0.5]));
    let mut stamp = Annot::new(
        Kind::Stamp {
            art: Art::Image(Arc::new(sample_round_stamp(64))),
            rotate: 0.0,
            signature: false,
        },
        [100.0, 80.0, 200.0, 180.0],
        [0.0; 3],
    );
    stamp.opacity = 0.8;
    v.push(stamp);
    v
}

/// What must survive a save and a read.
fn same(a: &Annot, b: &Annot) {
    let close = |x: f32, y: f32| (x - y).abs() < 0.02;
    assert_eq!(a.id, b.id);
    assert_eq!(a.contents, b.contents, "{:?}", a.kind);
    assert_eq!(a.author, b.author);
    assert_eq!(a.reply_to, b.reply_to);
    assert!(
        a.rect.iter().zip(b.rect).all(|(x, y)| (x - y).abs() < 0.05),
        "{:?} {:?} {:?}",
        a.kind,
        a.rect,
        b.rect
    );
    assert!(
        a.color.iter().zip(b.color).all(|(x, y)| close(*x, y)),
        "{:?}",
        a.kind
    );
    match (&a.kind, &b.kind) {
        (
            Kind::Stamp { rotate: r1, .. },
            Kind::Stamp {
                rotate: r2,
                art: Art::Keep,
                ..
            },
        ) => {
            assert!(close(*r1, *r2));
        }
        (x, y) => assert_eq!(x, y),
    }
    assert!(close(a.opacity, b.opacity));
}

#[test]
fn every_kind_writes_and_reads_back() {
    let mut pdf = Pdf::from_bytes(&sample_pdf(1), None).unwrap();
    let marks = every_kind();
    annot::apply_page(&mut pdf, 0, &marks).unwrap();
    let bytes = save(pdf);
    dump("every-kind.pdf", &bytes);
    assert!(qpdf_ok(&bytes));
    let back = annot::read_page(&Pdf::from_bytes(&bytes, None).unwrap(), 0);
    assert_eq!(back.len(), marks.len(), "pop-ups aren't listed");
    for (a, b) in marks.iter().zip(&back) {
        same(a, b);
        assert!(!b.dirty);
    }
    // Poppler draws them: the highlight is yellow over the white page, the
    // square's fill pale yellow, the line black.
    if let Some(img) = poppler(&bytes) {
        let h = 800.0;
        let [r, g, b] = at(&img, 150.0, 710.0, h);
        assert!(r > 200 && g > 180 && b < 80, "highlight {r} {g} {b}");
        let [r, g, b] = at(&img, 350.0, 330.0, h);
        assert!(r > 240 && g > 240 && b < 230, "square fill {r} {g} {b}");
        let [r, _, _] = at(&img, 300.0, 300.5, h);
        assert!(r > 150, "square border");
        // The round stamp: red ring, page showing through outside the circle.
        let [r, g, b] = at(&img, 150.0, 177.0, h);
        assert!(r > 150 && g < 120 && b < 120, "stamp ring {r} {g} {b}");
        assert_eq!(
            at(&img, 102.0, 82.0, h),
            [255, 255, 255],
            "transparent corner"
        );
        // The reply has no appearance of its own.
    }
}

#[test]
fn edits_keep_untouched_annotations_and_remove_deleted_ones() {
    let mut pdf = Pdf::from_bytes(&sample_pdf(1), None).unwrap();
    annot::apply_page(&mut pdf, 0, &every_kind()).unwrap();
    let bytes = save(pdf);
    let pdf = Pdf::from_bytes(&bytes, None).unwrap();
    let page = pdf.document().get_pages()[&1];
    let ids_before: Vec<_> = annot::page_annots(pdf.document(), page)
        .into_iter()
        .map(|(id, _)| id.unwrap())
        .collect();
    let mut marks = annot::read_page(&pdf, 0);
    // Delete the note (its replies and pop-up go too), move the square,
    // recolour the text box.
    marks.retain(|m| m.kind != Kind::Note && !matches!(m.kind, Kind::State(_)));
    let sq = marks.iter_mut().find(|m| m.kind == Kind::Square).unwrap();
    sq.translate(10.0, -5.0);
    let ft = marks
        .iter_mut()
        .find(|m| matches!(m.kind, Kind::FreeText { .. }))
        .unwrap();
    ft.color = [0.8, 0.0, 0.0];
    ft.touch();
    let mut pdf = pdf;
    annot::apply_page(&mut pdf, 0, &marks).unwrap();
    let after: Vec<_> = annot::page_annots(pdf.document(), page)
        .into_iter()
        .map(|(id, d)| (id.unwrap(), d))
        .collect();
    assert_eq!(after.len(), marks.len(), "no pop-up left");
    // Untouched ones are the same objects; the highlight's appearance too.
    assert_eq!(after[0].0, ids_before[0]);
    assert_eq!(
        after[0].1.get(b"AP").unwrap(),
        annot::page_annots(&Pdf::from_bytes(&bytes, None).unwrap().doc, page)[0]
            .1
            .get(b"AP")
            .unwrap()
    );
    let bytes = save(pdf);
    assert!(qpdf_ok(&bytes));
    let back = annot::read_page(&Pdf::from_bytes(&bytes, None).unwrap(), 0);
    let sq = back.iter().find(|m| m.kind == Kind::Square).unwrap();
    assert_eq!(sq.rect, [310.0, 295.0, 410.0, 355.0]);
    let ft = back
        .iter()
        .find(|m| matches!(m.kind, Kind::FreeText { .. }))
        .unwrap();
    assert_eq!(ft.color, [0.8, 0.0, 0.0]);
}

#[test]
fn other_annotations_stay_as_they_are() {
    let bytes = crate::testing::sample_pdf_linked(2);
    let mut pdf = Pdf::from_bytes(&bytes, None).unwrap();
    let mut marks = annot::read_page(&pdf, 0);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].kind, Kind::Other("Link".into()));
    marks.push(Annot::new(
        Kind::Square,
        [10.0, 10.0, 50.0, 50.0],
        [1.0, 0.0, 0.0],
    ));
    annot::apply_page(&mut pdf, 0, &marks).unwrap();
    let page = pdf.document().get_pages()[&1];
    let list = annot::page_annots(pdf.document(), page);
    assert_eq!(list.len(), 2);
    assert!(list[0].1.has(b"Dest"), "the link keeps its target");
}

#[test]
fn turned_stamps_and_layers() {
    let mut pdf = Pdf::from_bytes(&sample_pdf(1), None).unwrap();
    let stamp = Annot::new(
        Kind::Stamp {
            art: Art::Image(Arc::new(sample_round_stamp(32))),
            rotate: 30.0,
            signature: false,
        },
        [200.0, 300.0, 300.0, 340.0],
        [0.0; 3],
    );
    let b = stamp.bounds();
    assert!(b[2] - b[0] > 100.0 && b[3] - b[1] > 40.0);
    annot::apply_page(&mut pdf, 0, std::slice::from_ref(&stamp)).unwrap();
    let bytes = save(pdf);
    let back = annot::read_page(&Pdf::from_bytes(&bytes, None).unwrap(), 0);
    same(&stamp, &back[0]);
    // A kept stamp moved and turned again keeps its picture.
    let mut kept = back[0].clone();
    kept.translate(50.0, 0.0);
    if let Kind::Stamp { rotate, .. } = &mut kept.kind {
        *rotate = 90.0;
    }
    let mut pdf = Pdf::from_bytes(&bytes, None).unwrap();
    annot::apply_page(&mut pdf, 0, std::slice::from_ref(&kept)).unwrap();
    let bytes2 = save(pdf);
    assert!(qpdf_ok(&bytes2));
    let again = annot::read_page(&Pdf::from_bytes(&bytes2, None).unwrap(), 0);
    same(&kept, &again[0]);
    // The layer of a page: just its annotations, on a page of its size.
    let pdf = Pdf::from_bytes(&bytes, None).unwrap();
    let layer = annot::layer(&pdf, 0, &back, &HashMap::new()).unwrap();
    let l = Pdf::from_bytes(&layer, None).unwrap();
    assert_eq!(annot::read_page(&l, 0).len(), 1);
    assert_eq!(crate::page_size(&l, 0), Some((601.0, 800.0)));
}

#[test]
fn flatten_draws_stamps_into_the_page() {
    let mut pdf = Pdf::from_bytes(&sample_pdf(1), None).unwrap();
    let marks = every_kind();
    annot::apply_page(&mut pdf, 0, &marks).unwrap();
    let n = annot::flatten(&mut pdf, annot::FlattenWhat::StampsAndSignatures).unwrap();
    assert_eq!(n, 1);
    let bytes = save(pdf);
    dump("flattened.pdf", &bytes);
    assert!(qpdf_ok(&bytes));
    let back = annot::read_page(&Pdf::from_bytes(&bytes, None).unwrap(), 0);
    assert_eq!(back.len(), marks.len() - 1);
    assert!(!back.iter().any(|m| matches!(m.kind, Kind::Stamp { .. })));
    if let Some(img) = poppler(&bytes) {
        let [r, g, _] = at(&img, 150.0, 177.0, 800.0);
        assert!(r > 150 && g < 120, "flattened stamp drawn");
        assert_eq!(at(&img, 102.0, 82.0, 800.0), [255, 255, 255]);
    }
}

#[test]
fn forms_fill_save_and_read_back() {
    let bytes = sample_form_pdf();
    assert!(qpdf_ok(&bytes));
    let mut pdf = Pdf::from_bytes(&bytes, None).unwrap();
    let f = form::read_form(&pdf).unwrap();
    let names: Vec<&str> = f.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["name", "notes", "agree", "size", "city", "fruit"]);
    assert!(!f.xfa_only());
    assert_eq!(
        f.field("name").unwrap().value,
        FieldValue::Text("Ann".into())
    );
    assert_eq!(
        f.field("size").unwrap().value,
        FieldValue::State("M".into())
    );
    assert_eq!(f.field("size").unwrap().widgets.len(), 3);
    assert_eq!(
        f.field("agree").unwrap().widgets[0].on_state.as_deref(),
        Some("Yes")
    );
    assert!(matches!(
        f.field("city").unwrap().kind,
        FieldKind::Combo { edit: false }
    ));
    let mut values = HashMap::new();
    values.insert("name".to_owned(), FieldValue::Text("Budi Santoso".into()));
    values.insert(
        "notes".to_owned(),
        FieldValue::Text("Line one of the notes\nand two".into()),
    );
    values.insert("agree".to_owned(), FieldValue::State("Yes".into()));
    values.insert("size".to_owned(), FieldValue::State("L".into()));
    values.insert(
        "city".to_owned(),
        FieldValue::Choice(vec!["Bandung".into()]),
    );
    values.insert("fruit".to_owned(), FieldValue::Choice(vec!["pear".into()]));
    form::set_values(&mut pdf, &values).unwrap();
    let out = save(pdf);
    dump("form-filled.pdf", &out);
    assert!(qpdf_ok(&out));
    let back = form::read_form(&Pdf::from_bytes(&out, None).unwrap()).unwrap();
    for (k, v) in &values {
        assert_eq!(&back.field(k).unwrap().value, v, "{k}");
    }
    // Every widget has an appearance (no NeedAppearances needed).
    let doc = Pdf::from_bytes(&out, None).unwrap();
    let page = doc.document().get_pages()[&1];
    for (_, d) in annot::page_annots(doc.document(), page) {
        assert!(d.has(b"AP"), "{d:?}");
    }
    // Reset: the defaults (no /DV here: empty and off).
    let defaults = back.defaults();
    assert_eq!(defaults["agree"], FieldValue::State("Off".into()));
    if let Some(img) = poppler(&out) {
        // Text in the name field: some dark pixels.
        let dark = (74..200).any(|x| {
            let p = at(&img, x as f32, 610.0, 800.0);
            p[0] < 100
        });
        assert!(dark, "the filled name shows");
        // The check box is checked: dark pixels in it.
        let checked =
            (74..88).any(|x| (462..476).any(|y| at(&img, x as f32, y as f32, 800.0)[0] < 100));
        assert!(checked, "the check box shows its check");
    }
}

#[test]
fn layers_show_filled_values() {
    let pdf = Pdf::from_bytes(&sample_form_pdf(), None).unwrap();
    let marks = annot::read_page(&pdf, 0);
    let mut values = HashMap::new();
    values.insert("name".to_owned(), FieldValue::Text("Citra".into()));
    let layer = annot::layer(&pdf, 0, &marks, &values).unwrap();
    assert!(qpdf_ok(&layer));
    if let Some(img) = poppler(&layer) {
        let dark = (74..200).any(|x| at(&img, x as f32, 610.0, 800.0)[0] < 100);
        assert!(dark);
    }
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/signed")
            .join(name),
    )
    .unwrap()
}

/// pyHanko's verdict on every signature of `bytes` (made by
/// scripts/pdf/signed_fixtures.py); `None` when uv or the network isn't
/// there. Runs only with KANTOR_PYHANKO=1 (it downloads pyHanko once).
fn pyhanko_ok(bytes: &[u8], strict: bool) -> Option<bool> {
    std::env::var_os("KANTOR_PYHANKO")?;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signed.pdf");
    std::fs::write(&path, bytes).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
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

#[test]
fn signed_files_get_incremental_updates() {
    let original = fixture("signed-approval.pdf");
    let pdf = Pdf::from_bytes(&original, None).unwrap();
    let sigs = crate::sig::signatures(&pdf).unwrap();
    assert_eq!(sigs.count, 1);
    assert!(sigs.allows_annotations());
    let mut marks = annot::read_page(&pdf, 0);
    let n = marks.len();
    let mut note = Annot::new(Kind::Note, [400.0, 700.0, 420.0, 720.0], [1.0, 0.85, 0.2]);
    note.contents = "Checked".into();
    marks.push(note);
    marks.push(Annot::new(
        Kind::Stamp {
            art: Art::Image(Arc::new(sample_round_stamp(48))),
            rotate: 0.0,
            signature: false,
        },
        [300.0, 500.0, 400.0, 600.0],
        [0.0; 3],
    ));
    let out =
        crate::sig::incremental(&original, None, |p| annot::apply_page(p, 0, &marks)).unwrap();
    assert!(
        out.starts_with(&original),
        "the signed bytes are an exact prefix"
    );
    assert!(qpdf_ok(&out));
    let back = annot::read_page(&Pdf::from_bytes(&out, None).unwrap(), 0);
    assert_eq!(back.len(), n + 2);
    // The appended part holds only a few objects (no rewrite).
    let tail = &out[original.len()..];
    let objs = tail.windows(4).filter(|w| w == b" obj").count();
    assert!(objs < 12, "{objs} objects appended");
    if let Some(ok) = pyhanko_ok(&out, false) {
        assert!(ok, "pyhanko: the signature is intact");
    }
    // Form fill under a certification that allows it, likewise.
    let original = fixture("signed-certify-p2.pdf");
    let pdf = Pdf::from_bytes(&original, None).unwrap();
    let sigs = crate::sig::signatures(&pdf).unwrap();
    assert_eq!(sigs.docmdp, Some(2));
    assert!(sigs.allows_form_fill() && !sigs.allows_annotations());
    let mut values = HashMap::new();
    values.insert("name".to_owned(), FieldValue::Text("Budi".into()));
    let out = crate::sig::incremental(&original, None, |p| form::set_values(p, &values)).unwrap();
    assert!(out.starts_with(&original));
    assert!(qpdf_ok(&out));
    assert_eq!(
        form::read_form(&Pdf::from_bytes(&out, None).unwrap())
            .unwrap()
            .field("name")
            .unwrap()
            .value,
        FieldValue::Text("Budi".into())
    );
    if let Some(ok) = pyhanko_ok(&out, true) {
        assert!(ok, "pyhanko: form fill allowed by DocMDP 2");
    }
    for (name, p) in [("signed-certify-p1.pdf", 1), ("signed-certify-p3.pdf", 3)] {
        let pdf = Pdf::from_bytes(&fixture(name), None).unwrap();
        assert_eq!(crate::sig::signatures(&pdf).unwrap().docmdp, Some(p));
    }
    assert_eq!(
        crate::sig::signatures(&Pdf::from_bytes(&sample_pdf(1), None).unwrap()),
        None
    );
}

#[test]
fn layers_show_check_boxes_without_appearances() {
    let pdf = Pdf::from_bytes(&sample_form_pdf(), None).unwrap();
    let marks = annot::read_page(&pdf, 0);
    let mut values = HashMap::new();
    values.insert("agree".to_owned(), FieldValue::State("Yes".into()));
    let layer = annot::layer(&pdf, 0, &marks, &values).unwrap();
    if let Some(img) = poppler(&layer) {
        let checked =
            (74..88).any(|x| (462..476).any(|y| at(&img, x as f32, y as f32, 800.0)[0] < 100));
        assert!(checked, "the check shows in the layer");
    }
}

#[test]
fn xfa_form_with_only_a_signature_field_is_xfa_only() {
    // Like Canada's IMM 1344: dynamic XFA, the AcroForm holds one
    // signature field.
    let pdf = Pdf::from_bytes(&sample_form_pdf(), None).unwrap();
    let mut f = form::read_form(&pdf).unwrap();
    f.xfa = true;
    assert!(!f.xfa_only(), "AcroForm fields to fill beside the XFA");
    let field = f.fields[0].clone();
    f.fields = vec![form::Field {
        name: "Signature1".into(),
        kind: FieldKind::Signature,
        ..field
    }];
    assert!(f.xfa_only());
    f.xfa = false;
    assert!(!f.xfa_only());
}
