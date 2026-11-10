//! Compacts `data/presetShapeDefinitions.xml` (ECMA-376, see
//! `data/README.md`) into a line format that `src/geom/presets.rs` parses
//! lazily: guides, text rectangle and paths only (handles and connection
//! sites are dropped), one item per line.
//!
//! ```text
//! @name                       a preset
//! a name op arg…              avLst guide (adjust value default)
//! g name op arg…              gdLst guide
//! r l t r b                   text rectangle
//! p w h fill stroke extrusion path start (w/h 0 = shape size)
//! M x y | L x y | A wR hR stAng swAng | Q x y x y | C x y x y x y | Z
//! ```

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let src = "data/presetShapeDefinitions.xml";
    println!("cargo:rerun-if-changed={src}");
    println!("cargo:rerun-if-changed=build.rs");
    let xml = std::fs::read_to_string(src).expect("read preset shape definitions");
    let doc = roxmltree::Document::parse(&xml).expect("parse preset shape definitions");
    let mut out = String::new();
    for shape in doc.root_element().children().filter(|n| n.is_element()) {
        let _ = writeln!(out, "@{}", shape.tag_name().name());
        for part in shape.children().filter(|n| n.is_element()) {
            match part.tag_name().name() {
                list @ ("avLst" | "gdLst") => {
                    let tag = if list == "avLst" { 'a' } else { 'g' };
                    for gd in part.children().filter(|n| n.has_tag_name_local("gd")) {
                        let name = gd.attribute("name").expect("gd name");
                        let fmla = gd.attribute("fmla").expect("gd fmla");
                        let fmla = fmla.split_whitespace().collect::<Vec<_>>().join(" ");
                        let _ = writeln!(out, "{tag} {name} {fmla}");
                    }
                }
                "rect" => {
                    let a = |k| part.attribute(k).expect("rect side");
                    let _ = writeln!(out, "r {} {} {} {}", a("l"), a("t"), a("r"), a("b"));
                }
                "pathLst" => {
                    for path in part.children().filter(|n| n.is_element()) {
                        write_path(&mut out, path);
                    }
                }
                _ => {}
            }
        }
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("presets.txt");
    std::fs::write(dest, out).expect("write presets.txt");
}

fn write_path(out: &mut String, path: roxmltree::Node) {
    let flag = |k: &str| !matches!(path.attribute(k), Some("0" | "false"));
    let _ = writeln!(
        out,
        "p {} {} {} {} {}",
        path.attribute("w").unwrap_or("0"),
        path.attribute("h").unwrap_or("0"),
        path.attribute("fill").unwrap_or("norm"),
        u8::from(flag("stroke")),
        u8::from(flag("extrusionOk")),
    );
    for cmd in path.children().filter(|n| n.is_element()) {
        let pts = || {
            cmd.children()
                .filter(|n| n.has_tag_name_local("pt"))
                .map(|p| {
                    format!(
                        " {} {}",
                        p.attribute("x").unwrap(),
                        p.attribute("y").unwrap()
                    )
                })
                .collect::<String>()
        };
        match cmd.tag_name().name() {
            "moveTo" => {
                let _ = writeln!(out, "M{}", pts());
            }
            "lnTo" => {
                let _ = writeln!(out, "L{}", pts());
            }
            "quadBezTo" => {
                let _ = writeln!(out, "Q{}", pts());
            }
            "cubicBezTo" => {
                let _ = writeln!(out, "C{}", pts());
            }
            "arcTo" => {
                let a = |k| cmd.attribute(k).expect("arcTo attribute");
                let _ = writeln!(
                    out,
                    "A {} {} {} {}",
                    a("wR"),
                    a("hR"),
                    a("stAng"),
                    a("swAng")
                );
            }
            "close" => out.push_str("Z\n"),
            o => panic!("unknown path command {o}"),
        }
    }
}

trait LocalName {
    fn has_tag_name_local(&self, name: &str) -> bool;
}

impl LocalName for roxmltree::Node<'_, '_> {
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
}
