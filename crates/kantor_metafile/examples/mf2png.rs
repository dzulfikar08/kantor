//! `cargo run -p kantor_metafile --example mf2png -- in.emf out.png [out.svg]`:
//! render a WMF, EMF, PICT or BMP to PNG (and its SVG) for a look.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = args.as_slice() else {
        eprintln!("usage: mf2png IN OUT.png [OUT.svg]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(input).expect("read input");
    if let (Some(svg_out), Some(pic)) = (rest.first(), kantor_metafile::render(&bytes)) {
        std::fs::write(svg_out, pic.svg()).expect("write svg");
    }
    let t = std::time::Instant::now();
    let png = kantor_metafile::to_png(&bytes, 2048).expect("not a picture this crate reads");
    eprintln!(
        "{input}: {:?} in {:?}",
        kantor_metafile::natural_size(&bytes),
        t.elapsed()
    );
    std::fs::write(output, png).expect("write png");
}
