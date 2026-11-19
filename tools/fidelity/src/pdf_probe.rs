//! `pdf_probe IN OUTDIR [--pages N] [--ref DIR --dpi D]`: one real-world PDF
//! through Kantor's PDF viewer engine and page tools, as
//! `tools/realworld/pdf_sweep.py` needs it.
//!
//! Opens `IN` with `pdf_engine` (as the viewer does), renders the first `N`
//! pages (all when `N` is 0), extracts their text and searches it, reads the
//! outline and links. With `--ref DIR` each rendered page is compared with
//! `DIR/p-<n>.ppm` (poppler's `pdftoppm -r D -cropbox` of the same page) and
//! gets a difference score. Then the page-tool round trip: page 1 turned a
//! quarter, the last page deleted (when there are two or more), saved with
//! `kantor_pdf` as the viewer's Save does to `OUTDIR/saved.pdf`, and reopened
//! with `pdf_engine`.
//!
//! Prints one JSON object on stdout with each stage's time and error (a
//! panic is caught and reported as `panic: …`), per-page render times and
//! diff scores, and the peak memory.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use pdf_engine::{PdfDoc, Renderer, Turn, find_in};
use serde_json::{Value, json};

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic".into())
}

/// Run `f`, timing it; an error or a panic becomes the stage's `error`.
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

/// Peak resident memory of this process, MB (Linux).
fn peak_rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// A binary PPM (P6) image.
struct Ppm {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

fn read_ppm(path: &Path) -> Result<Ppm> {
    let data = std::fs::read(path)?;
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while i < data.len() && data[i].is_ascii_whitespace() {
            i += 1;
        }
        if data.get(i) == Some(&b'#') {
            while i < data.len() && data[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < data.len() && !data[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            bail!("short PPM header");
        }
        fields.push(String::from_utf8_lossy(&data[start..i]).into_owned());
    }
    i += 1;
    if fields[0] != "P6" || fields[3] != "255" {
        bail!("not an 8-bit P6 PPM");
    }
    let w: usize = fields[1].parse()?;
    let h: usize = fields[2].parse()?;
    let rgb = data
        .get(i..i + w * h * 3)
        .ok_or_else(|| anyhow!("short PPM data"))?
        .to_vec();
    Ok(Ppm { w, h, rgb })
}

/// How far a render is from poppler's: both turned grey and averaged over
/// 4 × 4 pixel cells (anti-aliasing and font hinting differ between
/// renderers), then `bad` is the share of cells (%) whose grey differs by
/// more than 48 of 255 and `mean` the mean absolute difference of the
/// cells. A size difference over 2 px is reported as such.
fn diff(r: &pdf_engine::Raster, p: &Ppm) -> Value {
    let (rw, rh) = (r.width as usize, r.height as usize);
    let size_off = rw.abs_diff(p.w).max(rh.abs_diff(p.h));
    let (w, h) = (rw.min(p.w), rh.min(p.h));
    const C: usize = 4;
    let (cw, ch) = (w / C, h / C);
    if cw == 0 || ch == 0 {
        return json!({ "size_off": size_off, "bad": null, "mean": null });
    }
    let grey = |c: [u8; 3]| (u32::from(c[0]) * 3 + u32::from(c[1]) * 6 + u32::from(c[2])) / 10;
    let mut bad = 0usize;
    let mut total = 0u64;
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut a, mut b) = (0u32, 0u32);
            for y in cy * C..cy * C + C {
                for x in cx * C..cx * C + C {
                    let i = (y * rw + x) * 4;
                    a += grey([r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]);
                    let j = (y * p.w + x) * 3;
                    b += grey([p.rgb[j], p.rgb[j + 1], p.rgb[j + 2]]);
                }
            }
            let d = a.abs_diff(b) / (C * C) as u32;
            total += u64::from(d);
            if d > 48 {
                bad += 1;
            }
        }
    }
    let n = (cw * ch) as f64;
    json!({
        "size_off": size_off,
        "bad": (bad as f64 / n * 1000.0).round() / 10.0,
        "mean": (total as f64 / n * 10.0).round() / 10.0,
    })
}

/// Write a render as PPM (to look at a page that differs).
fn write_ppm(r: &pdf_engine::Raster, path: &Path) -> Result<()> {
    let mut out = format!("P6\n{} {}\n255\n", r.width, r.height).into_bytes();
    out.reserve(r.rgba.len() / 4 * 3);
    for px in r.rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&px[..3]);
    }
    std::fs::write(path, out)?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: pdf_probe <file.pdf> <out dir> [--pages N] [--ref DIR --dpi D] [--keep]";
    let input = PathBuf::from(args.next().ok_or_else(|| anyhow!(usage))?);
    let out = PathBuf::from(args.next().ok_or_else(|| anyhow!(usage))?);
    let (mut max_pages, mut refs, mut dpi, mut keep) = (50usize, None::<PathBuf>, 36.0f32, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pages" => max_pages = args.next().ok_or_else(|| anyhow!(usage))?.parse()?,
            "--ref" => refs = Some(args.next().ok_or_else(|| anyhow!(usage))?.into()),
            "--dpi" => dpi = args.next().ok_or_else(|| anyhow!(usage))?.parse()?,
            "--keep" => keep = true,
            _ => bail!(usage),
        }
    }
    std::fs::create_dir_all(&out)?;
    std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));
    let mut report = json!({ "file": input.display().to_string() });
    let r = report.as_object_mut().expect("object");

    let data = Arc::new(std::fs::read(&input)?);
    let (doc, s) = stage(|| PdfDoc::open(data.clone(), "").map_err(|e| anyhow!("{e}")));
    r.insert("open".into(), s);
    if let Some(doc) = doc {
        let n = doc.page_count();
        r.insert("pages".into(), json!(n));
        let (outline, s) = stage(|| Ok(doc.outline().len()));
        r.insert("outline".into(), s);
        r.insert("outline_items".into(), json!(outline));
        let shown = if max_pages == 0 { n } else { n.min(max_pages) };
        let renderer = Renderer::new(&doc);
        let scale = dpi / 72.0;
        let mut pages = Vec::new();
        let (mut chars, mut hits, mut links) = (0usize, 0usize, 0usize);
        let mut first_word: Option<String> = None;
        for p in 0..shown {
            let (raster, s) = stage(|| {
                renderer
                    .render(p, scale, Turn(0))
                    .ok_or_else(|| anyhow!("no raster"))
            });
            let mut page = json!({ "render": s });
            if let (Some(raster), Some(dir)) = (&raster, &refs) {
                let ppm = dir.join(format!("p-{}.ppm", p + 1));
                match read_ppm(&ppm) {
                    Ok(ppm) => {
                        let d = diff(raster, &ppm);
                        if keep || d["bad"].as_f64().is_some_and(|b| b > 2.0) {
                            let _ = write_ppm(raster, &out.join(format!("k-{}.ppm", p + 1)));
                        }
                        page["diff"] = d;
                    }
                    Err(e) => page["diff"] = json!({ "error": format!("{e:#}") }),
                }
            }
            let (text, s) = stage(|| Ok(doc.text(p)));
            page["text"] = s;
            if let Some(text) = text {
                if keep {
                    let _ = std::fs::write(out.join(format!("t-{}.txt", p + 1)), &text.text);
                }
                chars += text.boxes.len();
                if first_word.is_none() {
                    first_word = text
                        .text
                        .split_whitespace()
                        .find(|w| w.chars().count() >= 4)
                        .map(str::to_owned);
                }
                if let Some(q) = &first_word {
                    let (found, s) = stage(|| Ok(find_in(p, &text, q, false, false).len()));
                    hits += found.unwrap_or(0);
                    if s["error"].is_string() {
                        page["find"] = s;
                    }
                }
            }
            let (l, s) = stage(|| Ok(doc.links(p).len()));
            links += l.unwrap_or(0);
            if s["error"].is_string() {
                page["links"] = s;
            }
            pages.push(page);
        }
        r.insert("rendered".into(), json!(shown));
        // Before the save, which reads the whole file with lopdf.
        r.insert("view_rss_mb".into(), json!(peak_rss_mb()));
        r.insert("chars".into(), json!(chars));
        r.insert("query".into(), json!(first_word));
        r.insert("hits".into(), json!(hits));
        r.insert("links".into(), json!(links));
        r.insert("page".into(), Value::Array(pages));

        // Page tools: page 1 turned, the last page deleted, saved.
        let saved = out.join("saved.pdf");
        let _ = std::fs::remove_file(&saved);
        let (bytes, s) = stage(|| {
            // As the viewer's Save does (`Source::lopdf`): a file that opened
            // without a password is encrypted with an empty user password.
            let pdf = match kantor_pdf::Pdf::from_bytes(&data, None) {
                Err(kantor_pdf::Error::Encrypted) => kantor_pdf::Pdf::from_bytes(&data, Some(""))?,
                r => r?,
            };
            let count = pdf.page_count();
            let keep_pages = if count > 1 { count - 1 } else { count };
            let picks: Vec<kantor_pdf::Pick> = (0..keep_pages)
                .map(|page| kantor_pdf::Pick {
                    source: 0,
                    page,
                    turn: if page == 0 { 90 } else { 0 },
                })
                .collect();
            let mut built = kantor_pdf::assemble(vec![pdf], &picks)?;
            let bytes = built.to_bytes_encrypted(true)?;
            kantor_pdf::write_atomic(&saved, &bytes)?;
            Ok(bytes)
        });
        r.insert("save".into(), s);
        if let Some(bytes) = bytes {
            r.insert("saved_kb".into(), json!(bytes.len() / 1024));
            let (again, s) =
                stage(|| PdfDoc::open(Arc::new(bytes), "").map_err(|e| anyhow!("{e}")));
            r.insert("reopen".into(), s);
            if let Some(again) = again {
                r.insert("pages_after".into(), json!(again.page_count()));
                let (_, s) = stage(|| {
                    Renderer::new(&again)
                        .render(0, scale, Turn(0))
                        .ok_or_else(|| anyhow!("no raster"))
                });
                r.insert("render_after".into(), s);
            }
        }
    }
    r.insert("peak_rss_mb".into(), json!(peak_rss_mb()));
    println!("{report}");
    Ok(())
}
