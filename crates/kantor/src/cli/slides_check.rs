//! `kantor slides check FILE.pptx [--json] [--slide N]` (and `kantor
//! slides build --check`): the findings of [`crate::agent::slides_check`].

use std::path::Path;

use crate::agent::slides_check;

/// Check `file`; returns the exit status: 0 without errors, 1 with
/// errors (or when the file can't be read), 2 for usage errors.
pub fn run(file: &Path, json: bool, slide: Option<&str>) -> i32 {
    if !file.is_file() {
        eprintln!("kantor slides check: {} is not a file", file.display());
        return 1;
    }
    let pres = match slides_engine::Presentation::open(file) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kantor slides check: {}: {e:#}", file.display());
            return 1;
        }
    };
    let only = match slide {
        None => None,
        Some(s) => match s.trim().parse::<usize>() {
            Ok(n) if (1..=pres.len()).contains(&n) => Some(n - 1),
            _ => {
                eprintln!(
                    "kantor slides check: --slide {s}: give a slide number from 1 to {}",
                    pres.len()
                );
                return 2;
            }
        },
    };
    let mut engine = slides_scene::Engine::new();
    let report = slides_check::check(&pres, &mut engine, only);
    if json {
        let mut v = report.json();
        v["file"] = serde_json::json!(file.display().to_string());
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        print!("{}", report.text(&file.display().to_string()));
    }
    i32::from(report.has_errors())
}
