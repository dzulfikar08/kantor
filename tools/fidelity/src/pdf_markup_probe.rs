//! `pdf_markup_probe IN OUTDIR --stamp PNG [--dpi D]`: one real-world PDF
//! through Kantor's PDF markup and form saving, as
//! `tools/realworld/pdf_markup_sweep.py` needs it.
//!
//! Picks the first page (of the first five) with text, and adds one of each
//! annotation kind the way the viewer's tools make them: a highlight over
//! the first word of four or more letters, a sticky note, a text box, an
//! ink drawing and an image stamp of `PNG`. Every AcroForm field that can be
//! filled gets a sample value. Then it saves as the viewer's Save does
//! (`pdf_ui` `model::build`: an incremental update when the file is
//! digitally signed, otherwise rebuilt and encrypted again like the
//! original) to `OUTDIR/saved.pdf`, and with stamps and signatures
//! flattened (Save As → Flatten) to `OUTDIR/flat.pdf`; DocMDP permissions
//! are respected as the viewer's tools do (no annotations on P = 1 or 2, no
//! form filling on P = 1). Both results are read back with `kantor_pdf` and
//! `pdf_engine`: every added annotation and field value must be there.
//!
//! Writes the chosen page rendered by Kantor before and after as
//! `OUTDIR/k-{before,saved,flat}.ppm` at `D` dpi, and prints one JSON object
//! with each annotation's box in those pixels so the sweep can check the
//! renders (Kantor's and poppler's) changed there.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use kantor_pdf::annot::{Annot, Art, Font, Kind};
use kantor_pdf::form::{FieldKind, FieldValue};
use pdf_engine::{PdfDoc, Renderer, Turn};
use serde_json::{Value, json};

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic".into())
}

fn stage<T>(f: impl FnOnce() -> Result<T>) -> (Option<T>, Value) {
    let start = Instant::now();
    let r = catch_unwind(AssertUnwindSafe(f));
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    match r {
        Ok(Ok(v)) => (Some(v), json!({ "ms": ms, "error": null })),
        Ok(Err(e)) => (None, json!({ "ms": ms, "error": format!("{e:#}") })),
        Err(p) => (
            None,
            json!({ "ms": ms, "error": format!("panic: {}", panic_text(&*p)) }),
        ),
    }
}

fn write_ppm(r: &pdf_engine::Raster, path: &Path) -> Result<()> {
    let mut out = format!("P6\n{} {}\n255\n", r.width, r.height).into_bytes();
    for px in r.rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&px[..3]);
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Page space ↔ user space of one page (`pdf_ui` `Source::to_user`).
struct Xf([f64; 6]);

impl Xf {
    fn to_user(&self, x: f32, y: f32) -> [f32; 2] {
        let m = self.0;
        let det = m[0] * m[3] - m[1] * m[2];
        if det.abs() < 1e-12 {
            return [x, y];
        }
        let (x, y) = (f64::from(x) - m[4], f64::from(y) - m[5]);
        [
            ((m[3] * x - m[2] * y) / det) as f32,
            ((-m[1] * x + m[0] * y) / det) as f32,
        ]
    }

    fn to_page(&self, x: f32, y: f32) -> [f32; 2] {
        let m = self.0;
        let (x, y) = (f64::from(x), f64::from(y));
        [
            (m[0] * x + m[2] * y + m[4]) as f32,
            (m[1] * x + m[3] * y + m[5]) as f32,
        ]
    }

    /// A page-space box as a user-space rect.
    fn user_rect(&self, r: [f32; 4]) -> [f32; 4] {
        let pts = [
            self.to_user(r[0], r[1]),
            self.to_user(r[2], r[1]),
            self.to_user(r[0], r[3]),
            self.to_user(r[2], r[3]),
        ];
        bounds(&pts)
    }

    /// A user-space rect as a page-space box in pixels at `scale`.
    fn px_rect(&self, r: [f32; 4], scale: f32) -> [f32; 4] {
        let pts = [
            self.to_page(r[0], r[1]),
            self.to_page(r[2], r[1]),
            self.to_page(r[0], r[3]),
            self.to_page(r[2], r[3]),
        ];
        let b = bounds(&pts);
        [b[0] * scale, b[1] * scale, b[2] * scale, b[3] * scale]
    }
}

fn bounds(pts: &[[f32; 2]]) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in pts {
        b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
    b
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

/// As `pdf_ui` `Source::lopdf`.
fn lopdf(bytes: &[u8]) -> kantor_pdf::Result<kantor_pdf::Pdf> {
    match kantor_pdf::Pdf::from_bytes(bytes, None) {
        Err(kantor_pdf::Error::Encrypted) => kantor_pdf::Pdf::from_bytes(bytes, Some("")),
        r => r,
    }
}

/// A sample value for a field, as a person filling the form would set it.
fn sample(f: &kantor_pdf::form::Field) -> Option<FieldValue> {
    if f.read_only {
        return None;
    }
    match &f.kind {
        FieldKind::Text {
            multiline, max_len, ..
        } => {
            let mut t = if *multiline {
                "Kantor sweep\nsecond line".to_string()
            } else {
                "Kantor 42".to_string()
            };
            if let Some(n) = max_len {
                t = t.chars().filter(|c| *c != '\n').take(*n).collect();
            }
            Some(FieldValue::Text(t))
        }
        FieldKind::Check | FieldKind::Radio => {
            // The last widget's on-state (a radio group's other choice).
            let on = f.widgets.iter().rev().find_map(|w| w.on_state.clone())?;
            Some(FieldValue::State(on))
        }
        FieldKind::Combo { edit } => match f.options.last() {
            Some((export, _)) => Some(FieldValue::Choice(vec![export.clone()])),
            None if *edit => Some(FieldValue::Choice(vec!["Kantor".into()])),
            None => None,
        },
        FieldKind::List { .. } => f
            .options
            .last()
            .map(|(export, _)| FieldValue::Choice(vec![export.clone()])),
        FieldKind::Push | FieldKind::Signature => None,
    }
}

fn kind_name(k: &Kind) -> &'static str {
    match k {
        Kind::Highlight(_) => "highlight",
        Kind::Note => "note",
        Kind::FreeText { .. } => "textbox",
        Kind::Ink(_) => "ink",
        Kind::Stamp { .. } => "stamp",
        _ => "other",
    }
}

/// What the edits are: per page, the full annotation list (the file's
/// own plus the new ones), and the form values.
struct Edits {
    page: u32,
    marks: Vec<Annot>,
    added: Vec<Annot>,
    fields: HashMap<String, FieldValue>,
}

impl Edits {
    /// `pdf_ui` `Plan::apply`.
    fn apply(&self, pdf: &mut kantor_pdf::Pdf) -> kantor_pdf::Result<()> {
        if !self.fields.is_empty() {
            kantor_pdf::form::set_values(pdf, &self.fields)?;
        }
        if !self.added.is_empty() {
            kantor_pdf::annot::apply_page(pdf, self.page, &self.marks)?;
        }
        Ok(())
    }
}

/// `pdf_ui` `model::build`: signed → incremental update; otherwise the
/// whole file again, encrypted like the original.
fn build(bytes: &[u8], edits: &Edits, signed: bool, flatten: bool) -> Result<(Vec<u8>, bool)> {
    if signed && !flatten {
        let out = kantor_pdf::sig::incremental(bytes, None, |pdf| edits.apply(pdf))?;
        return Ok((out, true));
    }
    let mut pdf = lopdf(bytes)?;
    edits.apply(&mut pdf)?;
    if flatten {
        kantor_pdf::annot::flatten(
            &mut pdf,
            kantor_pdf::annot::FlattenWhat::StampsAndSignatures,
        )?;
    }
    let count = pdf.page_count();
    let mut built = kantor_pdf::assemble(
        vec![pdf],
        &(0..count)
            .map(|page| kantor_pdf::Pick {
                source: 0,
                page,
                turn: 0,
            })
            .collect::<Vec<_>>(),
    )?;
    Ok((built.to_bytes_encrypted(true)?, false))
}

/// Read a saved file back as Kantor would: added annotations by id, field
/// values.
fn read_back(bytes: &[u8], edits: &Edits, flattened: bool) -> Result<Value> {
    let pdf = lopdf(bytes)?;
    let marks = kantor_pdf::annot::read_page(&pdf, edits.page);
    let mut missing = Vec::new();
    let mut wrong = Vec::new();
    for a in &edits.added {
        let stamp = matches!(a.kind, Kind::Stamp { .. });
        match marks.iter().find(|m| m.id == a.id) {
            None if flattened && stamp => {}
            None => missing.push(kind_name(&a.kind)),
            Some(_) if flattened && stamp => wrong.push("stamp still an annotation".to_string()),
            Some(m) => {
                if kind_name(&m.kind) != kind_name(&a.kind) {
                    wrong.push(format!("{} read as {:?}", kind_name(&a.kind), m.kind));
                } else if m.contents != a.contents {
                    wrong.push(format!(
                        "{} text {:?} != {:?}",
                        kind_name(&a.kind),
                        m.contents,
                        a.contents
                    ));
                } else if (m.rect[0] - a.rect[0]).abs() > 1.0 || (m.rect[3] - a.rect[3]).abs() > 1.0
                {
                    wrong.push(format!(
                        "{} rect {:?} != {:?}",
                        kind_name(&a.kind),
                        m.rect,
                        a.rect
                    ));
                }
            }
        }
    }
    let mut field_diff = Vec::new();
    if !edits.fields.is_empty() {
        match kantor_pdf::form::read_form(&pdf) {
            None => field_diff.push("form gone".to_string()),
            Some(form) => {
                for (name, v) in &edits.fields {
                    match form.field(name) {
                        None => field_diff.push(format!("{name}: gone")),
                        Some(f) if &f.value != v => {
                            field_diff.push(format!("{name}: {:?} != {v:?}", f.value))
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    field_diff.sort();
    let xfa = kantor_pdf::form::read_form(&pdf).is_some_and(|f| f.xfa);
    Ok(json!({
        "page_annots": marks.len(),
        "missing": missing,
        "wrong": wrong,
        "fields_differ": field_diff.len(),
        "field_diffs": field_diff.into_iter().take(8).collect::<Vec<_>>(),
        "xfa": xfa,
        "encrypted": pdf.was_encrypted(),
    }))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: pdf_markup_probe <file.pdf> <out dir> --stamp <png> [--dpi D]";
    let input = PathBuf::from(args.next().ok_or_else(|| anyhow!(usage))?);
    let out = PathBuf::from(args.next().ok_or_else(|| anyhow!(usage))?);
    let (mut stamp, mut dpi) = (None::<PathBuf>, 54.0f32);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--stamp" => stamp = Some(args.next().ok_or_else(|| anyhow!(usage))?.into()),
            "--dpi" => dpi = args.next().ok_or_else(|| anyhow!(usage))?.parse()?,
            _ => bail!(usage),
        }
    }
    let png = Arc::new(std::fs::read(stamp.ok_or_else(|| anyhow!(usage))?)?);
    let (pw, ph) = png_size(&png).ok_or_else(|| anyhow!("--stamp is not a PNG"))?;
    std::fs::create_dir_all(&out)?;
    std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));
    let scale = dpi / 72.0;
    let mut report = json!({ "file": input.display().to_string() });
    let r = report.as_object_mut().expect("object");
    let data = Arc::new(std::fs::read(&input)?);

    let (doc, s) = stage(|| PdfDoc::open(data.clone(), "").map_err(|e| anyhow!("{e}")));
    r.insert("open".into(), s);
    let Some(doc) = doc else {
        println!("{report}");
        return Ok(());
    };
    let (pdf, s) = stage(|| Ok(lopdf(&data)?));
    r.insert("load".into(), s);
    let Some(pdf) = pdf else {
        println!("{report}");
        return Ok(());
    };
    let sigs = kantor_pdf::sig::signatures(&pdf);
    r.insert(
        "signatures".into(),
        json!(sigs.map(|s| json!({ "count": s.count, "docmdp": s.docmdp }))),
    );
    let allow_annots = sigs.is_none_or(|s| s.allows_annotations());
    let allow_forms = sigs.is_none_or(|s| s.allows_form_fill());
    r.insert("encrypted".into(), json!(pdf.was_encrypted()));
    r.insert(
        "usage_rights".into(),
        json!(kantor_pdf::sig::usage_rights(&pdf)),
    );

    // The page: the first of five with a word to highlight.
    let n = doc.page_count();
    let mut chosen = None;
    for p in 0..n.min(5) {
        let text = doc.text(p);
        let mut at = 0;
        for w in text.text.split(['\n', ' ']) {
            if w.chars().filter(|c| c.is_alphanumeric()).count() >= 4 {
                chosen = Some((p, text.clone(), at..at + w.len(), w.to_string()));
                break;
            }
            at += w.len() + 1;
        }
        if chosen.is_some() {
            break;
        }
    }
    let page = chosen.as_ref().map_or(0, |c| c.0);
    let xf = Xf(doc.user_to_page(page));
    let (w, h) = doc.page_sizes()[page];
    r.insert("page".into(), json!(page));
    r.insert("word".into(), json!(chosen.as_ref().map(|c| c.3.clone())));

    let mut edits = Edits {
        page: page as u32,
        marks: kantor_pdf::annot::read_page(&pdf, page as u32),
        added: Vec::new(),
        fields: HashMap::new(),
    };
    r.insert("annots_before".into(), json!(edits.marks.len()));
    let new = |kind: Kind, rect: [f32; 4], color: [f32; 3]| {
        let mut a = Annot::new(kind, rect, color);
        a.author = "Kantor Sweep".into();
        a
    };
    if allow_annots {
        if let Some((_, text, range, word)) = &chosen {
            let quads: Vec<[f32; 8]> = text
                .rects(range.clone())
                .iter()
                .map(|r| {
                    let ul = xf.to_user(r[0], r[1]);
                    let ur = xf.to_user(r[2], r[1]);
                    let ll = xf.to_user(r[0], r[3]);
                    let lr = xf.to_user(r[2], r[3]);
                    [ul[0], ul[1], ur[0], ur[1], ll[0], ll[1], lr[0], lr[1]]
                })
                .collect();
            if !quads.is_empty() {
                let pts: Vec<[f32; 2]> = quads
                    .iter()
                    .flat_map(|q| [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]])
                    .collect();
                let mut a = new(Kind::Highlight(quads), bounds(&pts), [1.0, 0.85, 0.0]);
                a.contents = word.clone();
                edits.added.push(a);
            }
        }
        let note = xf.user_rect([w * 0.90, h * 0.03, w * 0.90 + 20.0, h * 0.03 + 20.0]);
        let mut a = new(Kind::Note, note, [1.0, 0.85, 0.0]);
        a.contents = "Kantor sweep note".into();
        edits.added.push(a);

        let tb = xf.user_rect([w * 0.55, h * 0.10, w * 0.92, h * 0.10 + 22.0]);
        let mut a = new(
            Kind::FreeText {
                font: Font::Helvetica,
                size: 12.0,
            },
            tb,
            [0.8, 0.0, 0.0],
        );
        a.fill = Some([1.0, 1.0, 0.85]);
        a.contents = "Kantor sweep text box".into();
        edits.added.push(a);

        let (x0, x1, y0, y1) = (w * 0.55, w * 0.92, h * 0.20, h * 0.30);
        let stroke: Vec<[f32; 2]> = (0..=8)
            .map(|i| {
                let x = x0 + (x1 - x0) * i as f32 / 8.0;
                let y = if i % 2 == 0 { y0 } else { y1 };
                xf.to_user(x, y)
            })
            .collect();
        let mut a = new(
            Kind::Ink(vec![stroke.clone()]),
            bounds(&stroke),
            [0.0, 0.2, 0.9],
        );
        a.width = 3.0;
        edits.added.push(a);

        let sw = w * 0.3;
        let sh = sw * ph as f32 / pw.max(1) as f32;
        let st = xf.user_rect([w * 0.62, h * 0.70, w * 0.62 + sw, h * 0.70 + sh]);
        let a = new(
            Kind::Stamp {
                art: Art::Image(png.clone()),
                rotate: 0.0,
                signature: false,
            },
            st,
            [0.0, 0.0, 0.0],
        );
        edits.added.push(a);
        edits.marks.extend(edits.added.iter().cloned());
    }
    r.insert(
        "added".into(),
        json!(
            edits
                .added
                .iter()
                .map(|a| json!({ "kind": kind_name(&a.kind), "px": xf.px_rect(a.bounds(), scale) }))
                .collect::<Vec<_>>()
        ),
    );

    let form = kantor_pdf::form::read_form(&pdf);
    if let Some(form) = &form {
        let fillable = form
            .fields
            .iter()
            .filter(|f| !matches!(f.kind, FieldKind::Push | FieldKind::Signature))
            .count();
        if allow_forms {
            for f in &form.fields {
                if !f.widgets.is_empty()
                    && let Some(v) = sample(f)
                {
                    edits.fields.insert(f.name.clone(), v);
                }
            }
        }
        r.insert(
            "form".into(),
            json!({
                "fields": form.fields.len(),
                "fillable": fillable,
                "filled": edits.fields.len(),
                "xfa": form.xfa,
                "xfa_only": form.xfa_only(),
            }),
        );
    }
    r.insert("allow_annots".into(), json!(allow_annots));
    r.insert("allow_forms".into(), json!(allow_forms));

    let renderer = Renderer::new(&doc);
    if let Some(before) = renderer.render(page, scale, Turn(0)) {
        write_ppm(&before, &out.join("k-before.ppm"))?;
    }
    drop(renderer);

    if edits.added.is_empty() && edits.fields.is_empty() {
        r.insert("saved".into(), json!(null));
        println!("{report}");
        return Ok(());
    }
    // `pdf_ui` `Plan::incremental`: signed, or Reader usage rights.
    let signed = sigs.is_some() || kantor_pdf::sig::usage_rights(&pdf);
    for (name, flatten) in [("saved", false), ("flat", true)] {
        let path = out.join(format!("{name}.pdf"));
        let _ = std::fs::remove_file(&path);
        let (built, s) = stage(|| build(&data, &edits, signed, flatten));
        let mut res = json!({ "save": s });
        if let Some((bytes, incremental)) = built {
            kantor_pdf::write_atomic(&path, &bytes)?;
            res["incremental"] = json!(incremental);
            res["prefix_ok"] = json!(bytes.starts_with(&data));
            res["kb"] = json!(bytes.len() / 1024);
            let (rb, s) = stage(|| read_back(&bytes, &edits, flatten));
            res["readback"] = rb.unwrap_or(s);
            let bytes = Arc::new(bytes);
            let (again, s) = stage(|| PdfDoc::open(bytes.clone(), "").map_err(|e| anyhow!("{e}")));
            res["reopen"] = s;
            if let Some(again) = again {
                res["pages_after"] = json!(again.page_count());
                let (raster, s) = stage(|| {
                    Renderer::new(&again)
                        .render(page, scale, Turn(0))
                        .ok_or_else(|| anyhow!("no raster"))
                });
                res["render"] = s;
                if let Some(raster) = raster {
                    write_ppm(&raster, &out.join(format!("k-{name}.ppm")))?;
                }
            }
        }
        r.insert(name.into(), res);
    }
    println!("{report}");
    Ok(())
}
