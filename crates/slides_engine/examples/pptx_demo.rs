//! Write a small edited deck (new presentation + every kind of edit) for
//! checking with other tools: `cargo run -p slides_engine --example
//! pptx_demo -- out.pptx [image.png]`. With an input `.pptx` as the second
//! argument instead, it opens it, appends a slide and saves a copy.

use slides_engine::*;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "kantor-demo.pptx".into());
    let input = args.next();
    let mut p = match input.as_deref() {
        Some(path) if path.ends_with(".pptx") => Presentation::open(std::path::Path::new(path))?,
        _ => Presentation::new(),
    };
    let title = p
        .slide(0)
        .shapes
        .iter()
        .position(|s| s.text_editable() && s.ph.is_some());
    if let Some(t) = title {
        p.insert_text((0, t), Selection::default(), "Made with Kantor", None)?;
    }
    let s = p.new_slide(Some(p.len() - 1))?;
    let tb = p.add_text_box(s)?;
    p.insert_text((s, tb), Selection::default(), "Hello\nWorld", None)?;
    p.add_geometry(s, Geom::Rect)?;
    p.add_geometry(s, Geom::Ellipse)?;
    if let Some(img) = input.filter(|i| !i.ends_with(".pptx")) {
        p.add_picture(s, std::fs::read(img)?)?;
    }
    p.duplicate_slide(s)?;
    p.save_as(std::path::Path::new(&out))?;
    let problems = package_problems(&std::fs::read(&out)?);
    println!(
        "wrote {out}: {} slides, {} package problems",
        p.len(),
        problems.len()
    );
    for pr in problems {
        println!("  {pr}");
    }
    Ok(())
}
