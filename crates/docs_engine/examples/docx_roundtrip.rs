//! Open a `.docx`, print what the model holds, and save it to a new file.
//!
//! `cargo run -p docs_engine --example docx_roundtrip -- in.docx out.docx`
//! (with no input, writes a small new document to `out.docx`).

use docs_engine::{Document, Paragraph, RunProps, Selection};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut doc, out) = match args.as_slice() {
        [input, out] => (Document::open(input.as_ref())?, out.clone()),
        [out] => {
            let mut doc = Document::from_paragraphs(vec![
                Paragraph::new("Kantor Docs").with_style("Heading1"),
                Paragraph::new("A paragraph written by docs_engine."),
            ]);
            let bold = RunProps {
                bold: Some(true),
                ..RunProps::default()
            };
            let end = doc.end();
            doc.insert_text(Selection::caret(end), " Bold tail.", Some(bold));
            (doc, out.clone())
        }
        _ => anyhow::bail!("usage: docx_roundtrip [in.docx] out.docx"),
    };
    println!(
        "{} paragraphs, {} words, page {}×{} twips",
        doc.len(),
        doc.word_count(),
        doc.page.width,
        doc.page.height
    );
    for (i, p) in doc.paragraphs().iter().enumerate().take(12) {
        let style = p.props.style.as_deref().unwrap_or("-");
        println!("  {i:>3} [{style}] {} runs: {:.60}", p.runs.len(), p.text);
    }
    if doc.has_unsupported_content() {
        println!("unsupported: {}", doc.unsupported_content().join(", "));
    }
    doc.save_as(out.as_ref())?;
    println!("saved {out}");
    Ok(())
}
