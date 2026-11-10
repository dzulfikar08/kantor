//! Render a `.pptx` to PDF (and to PNGs when `pdftoppm` is installed) to
//! eyeball the output:
//!
//! ```text
//! cargo run --release -p slides_pdf --example render -- deck.pptx [OUT_DIR] [--hidden]
//! ```
//!
//! Writes `OUT_DIR/NAME.pdf` and `OUT_DIR/NAME-N.png` (default OUT_DIR:
//! `target/slides-render`). Keep renders of private decks out of the repo.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let hidden = args.iter().any(|a| a == "--hidden");
    args.retain(|a| a != "--hidden");
    let Some(input) = args.first().map(PathBuf::from) else {
        eprintln!("usage: render DECK.pptx [OUT_DIR] [--hidden]");
        std::process::exit(2);
    };
    let out = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/slides-render"));
    std::fs::create_dir_all(&out)?;
    let stem = input
        .file_stem()
        .map_or("deck".into(), |s| s.to_string_lossy().into_owned());
    let t0 = Instant::now();
    let pres = slides_engine::Presentation::open(&input)?;
    let mut engine = slides_scene::Engine::new();
    let builder = slides_scene::Builder::new();
    let t1 = Instant::now();
    let deck = slides_pdf::deck(&pres, &builder, &mut engine, hidden);
    let t2 = Instant::now();
    let bytes = slides_pdf::write(&deck)?;
    let t3 = Instant::now();
    let pdf = out.join(format!("{stem}.pdf"));
    slides_pdf::write_file(&pdf, &bytes)?;
    println!(
        "{} slides ({} printed) → {} ({} KB); open {:?}, scenes {:?}, encode {:?}",
        pres.len(),
        deck.slides.len(),
        pdf.display(),
        bytes.len() / 1024,
        t1 - t0,
        t2 - t1,
        t3 - t2
    );
    let png = Command::new("pdftoppm")
        .args(["-png", "-r", "60"])
        .arg(&pdf)
        .arg(out.join(&stem))
        .status();
    match png {
        Ok(s) if s.success() => println!("PNGs in {}", out.display()),
        _ => println!("(pdftoppm not available: no PNGs)"),
    }
    Ok(())
}
