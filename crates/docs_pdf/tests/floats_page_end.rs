//! `floats_page_end.docx` (scripts/corpus/gen_docx_more.py): square,
//! tight and through pictures that move with the text and don't fit at the
//! bottom of a page go to the next page with their paragraph in Word 2013+
//! mode; a margin-relative one stays; in older modes only top-and-bottom
//! objects move.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use docs_engine::{Document, Object};
use docs_layout::Engine;

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/floats_page_end.docx")
}

/// (page of the picture, page of its paragraph's first line, picture
/// rect, top of that line) for the picture named `name`.
fn placed(doc: &Document, name: &str) -> (usize, usize, [f32; 4], f32) {
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(doc, &mut engine);
    let pages = &laid.paged.pages;
    let (pi, index) = doc
        .paragraphs()
        .iter()
        .enumerate()
        .find_map(|(pi, p)| {
            p.objects().into_iter().find_map(|(i, o)| match o {
                Object::Float(f) if f.name == name => Some((pi, i)),
                _ => None,
            })
        })
        .unwrap_or_else(|| panic!("{name} not found"));
    let (fp, rect) = pages
        .iter()
        .enumerate()
        .find_map(|(n, p)| {
            p.floats
                .iter()
                .find(|pf| (pf.para, pf.index) == (pi, index))
                .map(|pf| (n, pf.rect))
        })
        .expect("placed");
    let (lp, top) = pages
        .iter()
        .enumerate()
        .find_map(|(n, p)| p.lines.iter().find(|l| l.para == pi).map(|l| (n, l.top)))
        .expect("anchor line");
    (fp, lp, rect, top)
}

#[test]
fn wrapping_pictures_move_to_the_next_page_with_their_paragraph() {
    let doc = Document::open(&path()).unwrap();
    assert_eq!(doc.compat_mode(), 15);
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let pages = &laid.paged.pages;
    // Each moved picture gets a page of its own: 2 + 2 + 2 + 1.
    assert_eq!(pages.len(), 7, "{}", pages.len());
    for (name, page) in [
        ("Square picture", 1),
        ("Tight picture", 3),
        ("Through picture", 5),
    ] {
        let (fp, lp, rect, top) = placed(&doc, name);
        assert_eq!((fp, lp), (page, page), "{name}");
        let setup = &pages[page].setup;
        // At the top of the next page, inside the text area.
        assert!((rect[1] - setup.margin_top).abs() < 0.5, "{name} {rect:?}");
        assert!(
            rect[1] + rect[3] <= setup.height - setup.margin_bottom,
            "{name}"
        );
        // Its paragraph went with it, beside it (square wrapping).
        assert!((top - setup.margin_top).abs() < 0.5, "{name} {top}");
        // The page before ends with the 40th line of filler.
        assert!(
            pages[page - 1]
                .lines
                .iter()
                .all(|l| l.para != pages[page].lines[0].para)
        );
    }
    // Positioned from the margin: stays on its page, over the bottom margin.
    let (fp, lp, rect, _) = placed(&doc, "Margin picture");
    assert_eq!((fp, lp), (6, 6));
    let setup = &pages[6].setup;
    assert!(rect[1] + rect[3] > setup.height - setup.margin_bottom + 1.0);
}

/// The fixture with `compatibilityMode` set to `mode`, written to a file.
fn with_mode(mode: u32) -> PathBuf {
    let mut src = zip::ZipArchive::new(std::fs::File::open(path()).unwrap()).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "kantor-floats-page-end-{}-{mode}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("floats_page_end.docx");
    let mut out = zip::ZipWriter::new(std::fs::File::create(&out_path).unwrap());
    for i in 0..src.len() {
        let mut f = src.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if name == "word/settings.xml" {
            let text = String::from_utf8(data).unwrap();
            assert!(text.contains("w:val=\"15\""));
            data = text
                .replace("w:val=\"15\"", &format!("w:val=\"{mode}\""))
                .into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&data).unwrap();
    }
    out.finish().unwrap();
    out_path
}

#[test]
fn older_compatibility_modes_leave_square_pictures_in_the_margin() {
    let p = with_mode(14);
    let doc = Document::open(&p).unwrap();
    assert_eq!(doc.compat_mode(), 14);
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert_eq!(laid.paged.pages.len(), 4);
    for (name, page) in [
        ("Square picture", 0),
        ("Tight picture", 1),
        ("Through picture", 2),
    ] {
        let (fp, lp, ..) = placed(&doc, name);
        assert_eq!((fp, lp), (page, page), "{name}");
    }
    let _ = std::fs::remove_dir_all(Path::new(&p).parent().unwrap());
}
