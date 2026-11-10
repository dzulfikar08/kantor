//! The preset shapes of ECMA-376 (`presetShapeDefinitions.xml`, see
//! `data/README.md` for source and licence: © Ecma International,
//! redistributable), compacted by `build.rs` and parsed on first use.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use super::{Arg, FillMode, Guide, PathDef, PathDefCmd, ShapeDef};

static COMPACT: &str = include_str!(concat!(env!("OUT_DIR"), "/presets.txt"));

fn table() -> &'static HashMap<&'static str, Arc<ShapeDef>> {
    static TABLE: OnceLock<HashMap<&'static str, Arc<ShapeDef>>> = OnceLock::new();
    TABLE.get_or_init(|| parse(COMPACT))
}

/// The definition of preset `name` (`a:prstGeom/@prst`).
pub fn preset(name: &str) -> Option<Arc<ShapeDef>> {
    table().get(name).cloned()
}

/// Every preset name, sorted.
pub fn preset_names() -> Vec<&'static str> {
    let mut v: Vec<_> = table().keys().copied().collect();
    v.sort_unstable();
    v
}

fn parse(src: &'static str) -> HashMap<&'static str, Arc<ShapeDef>> {
    let mut out = HashMap::new();
    let mut name = None;
    let mut def = ShapeDef::default();
    for line in src.lines() {
        if let Some(n) = line.strip_prefix('@') {
            if let Some(prev) = name.replace(n) {
                out.insert(prev, Arc::new(std::mem::take(&mut def)));
            }
            continue;
        }
        let mut t = line.split(' ');
        let tag = t.next().unwrap_or_default();
        let args: Vec<&str> = t.collect();
        let a = |i: usize| Arg::parse(args.get(i).copied().unwrap_or("0"));
        match tag {
            "a" | "g" => {
                if let Some(g) = args
                    .split_first()
                    .and_then(|(n, f)| Guide::new(n, &f.join(" ")))
                {
                    if tag == "a" { &mut def.av } else { &mut def.gd }.push(g);
                }
            }
            "r" => def.rect = Some([a(0), a(1), a(2), a(3)]),
            "p" => def.paths.push(PathDef {
                w: args.first().and_then(|v| v.parse().ok()).unwrap_or(0),
                h: args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
                fill: FillMode::parse(args.get(2).copied().unwrap_or("norm")),
                stroke: args.get(3) != Some(&"0"),
                extrusion_ok: args.get(4) != Some(&"0"),
                cmds: Vec::new(),
            }),
            _ => {
                let Some(p) = def.paths.last_mut() else {
                    continue;
                };
                p.cmds.push(match tag {
                    "M" => PathDefCmd::MoveTo(a(0), a(1)),
                    "L" => PathDefCmd::LineTo(a(0), a(1)),
                    "A" => PathDefCmd::ArcTo([a(0), a(1), a(2), a(3)]),
                    "Q" => PathDefCmd::QuadTo([a(0), a(1), a(2), a(3)]),
                    "C" => PathDefCmd::CubicTo([a(0), a(1), a(2), a(3), a(4), a(5)]),
                    "Z" => PathDefCmd::Close,
                    _ => continue,
                });
            }
        }
    }
    if let Some(n) = name {
        out.insert(n, Arc::new(def));
    }
    out
}
