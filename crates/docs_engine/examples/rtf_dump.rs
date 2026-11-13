//! Print what an `.rtf` file imports as: `cargo run -p docs_engine --example rtf_dump -- FILE [--xml]`.
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::Path::new(&args[0]);
    if args.iter().any(|a| a == "--xml") {
        let import = docs_engine::rtf::to_docx(&std::fs::read(path)?)?;
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(import.docx))?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let mut s = String::new();
            if std::io::Read::read_to_string(&mut f, &mut s).is_ok() {
                println!("== {}\n{s}\n", f.name().unwrap_or_default());
            }
        }
        return Ok(());
    }
    let doc = docs_engine::Document::open(path)?;
    println!(
        "title {} notes {:?} unsupported {:?} preserved {:?}",
        doc.title(),
        doc.import_notes(),
        doc.unsupported_content(),
        doc.preserved_content()
    );
    println!("page {:?}", doc.page);
    let markers = doc.list_markers();
    for (i, p) in doc.paragraphs().iter().enumerate() {
        let r = doc.resolve_para(p);
        println!(
            "{i:3} {:?} style={:?} marker={:?} cell={:?} align={:?} | {:?}",
            p.runs.len(),
            p.props.style,
            markers.get(i).cloned().flatten(),
            p.cell.map(|c| (c.table, c.row, c.cell)),
            r.align,
            p.text
        );
    }
    for st in doc.stories() {
        let text: Vec<&str> = st
            .doc
            .paragraphs()
            .iter()
            .map(|p| p.text.as_str())
            .collect();
        println!("story {:?} {:?}", st.kind, text);
    }
    Ok(())
}
