//! Writes the test suite's sample report as `.docx` (heading, coloured
//! run, numbered list, picture, a 3 × 2 table, and `N` more paragraphs),
//! for trying `kantor convert` and Export as PDF by hand.
//!
//! `cargo run -p docs_pdf --example sample -- DIR [N]` → `DIR/report.docx`

#[path = "../tests/fixture/mod.rs"]
mod fixture;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let extra = args.next().and_then(|n| n.parse().ok()).unwrap_or(40);
    fixture::report(&dir, true, extra);
    println!("wrote {}", dir.join("report.docx").display());
}
