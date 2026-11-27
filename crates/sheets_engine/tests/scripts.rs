//! JavaScript and Python scripts stored in the workbook (a custom XML part):
//! saved, carried by later saves, replaced, removed, and the package stays
//! valid for Excel.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sheets_engine::Workbook;
use sheets_engine::scripts::{Script, ScriptLanguage};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scripts");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut data = Vec::new();
        e.read_to_end(&mut data).unwrap();
        out.insert(e.name().unwrap().into_owned(), data);
    }
    out
}

fn text(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(parts[name].clone()).unwrap()
}

fn js(name: &str, source: &str) -> Script {
    Script::new(name, ScriptLanguage::JavaScript, source)
}

#[test]
fn scripts_round_trip_through_saves() {
    let first = scratch("first.xlsx");
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "x").unwrap();
    assert!(!wb.has_scripts());
    let scripts = vec![
        js(
            "Report",
            "function go() {\r\n  Range(\"A1\").value = 1 < 2 && 3 > 2;\r\n}\n",
        ),
        Script::new(
            "Tools",
            ScriptLanguage::Python,
            "def go():\n    Range('A1').Value = 'é'\n",
        ),
    ];
    wb.set_scripts(scripts.clone());
    assert!(wb.is_dirty());
    wb.save_as(&first).unwrap();
    assert!(!wb.is_dirty());
    assert_eq!(wb.save_notes(), &[] as &[String]);

    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    let p = parts(&first);
    let item = text(&p, "customXml/item1.xml");
    assert!(item.contains("<scripts xmlns=\"https://kantor.paparanai.id/2026/scripts\">"));
    assert!(p.contains_key("customXml/itemProps1.xml"));
    assert!(text(&p, "customXml/_rels/item1.xml.rels").contains("itemProps1.xml"));
    assert!(text(&p, "xl/_rels/workbook.xml.rels").contains("../customXml/item1.xml"));
    let types = text(&p, "[Content_Types].xml");
    assert!(types.contains("/customXml/itemProps1.xml"));
    assert!(types.contains("customXmlProperties+xml"));

    // Reopened, and carried by a save that didn't change them.
    let mut wb = Workbook::open(&first).unwrap();
    assert_eq!(wb.scripts().unwrap(), scripts);
    wb.set_input(0, 1, 0, "more").unwrap();
    let second = scratch("second.xlsm");
    wb.save_as(&second).unwrap();
    let wb = Workbook::open(&second).unwrap();
    assert_eq!(wb.scripts().unwrap(), scripts);
    let p = parts(&second);
    assert_eq!(
        p.keys().filter(|k| k.starts_with("customXml/item")).count(),
        2,
        "{:?}",
        p.keys().collect::<Vec<_>>()
    );

    // Replaced: one scripts part, the new list.
    let mut wb = Workbook::open(&second).unwrap();
    let changed = vec![js("Other", "function other() {}\n")];
    wb.set_scripts(changed.clone());
    let third = scratch("third.xlsx");
    wb.save_as(&third).unwrap();
    let bytes = std::fs::read(&third).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    let wb = Workbook::open(&third).unwrap();
    assert_eq!(wb.scripts().unwrap(), changed);
    let p = parts(&third);
    let items: Vec<&String> = p
        .keys()
        .filter(|k| k.starts_with("customXml/") && !k.contains("_rels"))
        .collect();
    assert_eq!(items.len(), 2, "{items:?}");

    // Removed: no custom XML left, no dangling content types.
    let mut wb = Workbook::open(&third).unwrap();
    wb.set_scripts(Vec::new());
    let fourth = scratch("fourth.xlsx");
    wb.save_as(&fourth).unwrap();
    let bytes = std::fs::read(&fourth).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    let p = parts(&fourth);
    assert!(!p.keys().any(|k| k.starts_with("customXml/")));
    assert!(!text(&p, "[Content_Types].xml").contains("customXml"));
    assert!(!text(&p, "xl/_rels/workbook.xml.rels").contains("customXml"));
    let wb = Workbook::open(&fourth).unwrap();
    assert!(!wb.has_scripts());
}
