//! A save keeps the workbook's own theme (`xl/theme/theme1.xml`, with its
//! relationships and pictures) byte for byte, so theme colours, theme fonts
//! and the line/fill/effect styles shapes, pictures and charts refer to look
//! the same in Excel and in Kantor after the save. New workbooks get
//! IronCalc's theme.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::Workbook;
use sheets_engine::drawing::DrawingKind;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("theme_kept");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        out.insert(e.name().unwrap().into_owned(), b);
    }
    out
}

fn text(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8_lossy(parts.get(name).unwrap_or_else(|| panic!("no {name}"))).into_owned()
}

const THEME: &str = "xl/theme/theme1.xml";
const THEME_RELS: &str = "xl/theme/_rels/theme1.xml.rels";

/// The "Styled photo" picture's resolved outline.
fn outline(wb: &Workbook) -> String {
    wb.drawings(0)
        .into_iter()
        .find_map(|d| match &*d.kind {
            DrawingKind::Picture { outline, .. } if d.name == "Styled photo" => {
                outline.as_deref().map(str::to_owned)
            }
            _ => None,
        })
        .expect("Styled photo has an outline")
}

/// How the formatted cells look (theme colours resolved).
fn looks(wb: &Workbook) -> Vec<String> {
    [(0, 0), (2, 0), (2, 1), (3, 0), (3, 1)]
        .into_iter()
        .map(|(r, c)| format!("{:?}", wb.cell_style(0, r, c)))
        .collect()
}

/// The theme part a saved package's workbook points at, with its
/// relationships and the picture they point to.
fn assert_theme_kept(orig: &BTreeMap<String, Vec<u8>>, saved: &BTreeMap<String, Vec<u8>>) {
    assert!(text(orig, THEME) == text(saved, THEME), "theme changed");
    assert_eq!(text(orig, THEME_RELS), text(saved, THEME_RELS));
    let target = text(saved, THEME_RELS)
        .split("Target=\"../")
        .nth(1)
        .and_then(|t| t.split('"').next())
        .map(|t| format!("xl/{t}"))
        .unwrap();
    assert_eq!(
        orig.get("xl/media/image1.png"),
        saved.get(&target),
        "theme picture {target}"
    );
    let ct = text(saved, "[Content_Types].xml");
    assert_eq!(ct.matches("/xl/theme/theme1.xml").count(), 1, "{ct}");
    assert!(ct.contains("Extension=\"png\""), "{ct}");
    let wb_rels = text(saved, "xl/_rels/workbook.xml.rels");
    assert_eq!(wb_rels.matches("/theme\"").count(), 1, "{wb_rels}");
}

#[test]
fn save_keeps_the_files_theme() {
    let src = corpus("29_custom_theme.xlsx");
    let orig = parts(&src);
    let mut wb = Workbook::open(&src).unwrap();
    let line = outline(&wb);
    assert!(line.contains("w=\"22225\""), "{line}");
    let cells = looks(&wb);
    assert!(wb.theme_xml().unwrap().contains("Kantor Plum"));

    // A plain save, then an edit and a second save, then save -> undo ->
    // save: the theme never changes.
    let path = scratch("plain.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let first = parts(&path);
    assert_theme_kept(&orig, &first);
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());

    wb.set_input(0, 5, 0, "Q4").unwrap();
    wb.set_input(0, 5, 1, "160").unwrap();
    wb.save_as(&path).unwrap();
    let second = parts(&path);
    assert_theme_kept(&orig, &second);
    assert!(wb.undo().unwrap());
    wb.save_as(&path).unwrap();
    let third = parts(&path);
    assert_theme_kept(&orig, &third);
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());

    // Reopened, everything taken from the theme reads the same: the
    // picture's style outline (the theme's own 1.75 pt line, not IronCalc's
    // 1.5 pt), cell theme colours, the chart's theme.
    let back = Workbook::open(&path).unwrap();
    assert_eq!(outline(&back), line);
    assert_eq!(looks(&back), cells);
    assert!(back.theme_xml().as_deref() == Some(&*text(&orig, THEME)));
    let charts = back
        .drawings(0)
        .into_iter()
        .filter(|d| matches!(&*d.kind, DrawingKind::Chart { .. }))
        .count();
    assert_eq!(charts, 1);

    // Saved again from the reopened copy: still the same bytes.
    let again = scratch("again.xlsx");
    let mut back = back;
    back.save_as(&again).unwrap();
    assert_theme_kept(&orig, &parts(&again));
}

/// Only theme-less workbooks and new ones get IronCalc's theme.
#[test]
fn new_workbook_gets_ironcalcs_theme() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "x").unwrap();
    let path = scratch("new.xlsx");
    wb.save_as(&path).unwrap();
    let saved = parts(&path);
    assert!(text(&saved, THEME).contains("Aptos"));
    // Saved again (the saved file is now the original): the same theme.
    wb.save_as(&path).unwrap();
    assert!(text(&parts(&path), THEME) == text(&saved, THEME));
}

/// A theme with Office's look (the shapes fixture) is kept too: its line
/// styles are 0.5 / 1 / 1.5 pt, IronCalc's 1 / 1.5 / 2 pt.
#[test]
fn office_theme_line_widths_kept() {
    let src = corpus("28_shaped_pictures.xlsx");
    let orig = parts(&src);
    let mut wb = Workbook::open(&src).unwrap();
    let path = scratch("shaped.xlsx");
    wb.save_as(&path).unwrap();
    let saved = parts(&path);
    assert!(text(&orig, THEME) == text(&saved, THEME), "theme changed");
    assert!(!saved.contains_key(THEME_RELS));
}
