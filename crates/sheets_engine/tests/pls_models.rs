//! PLS-SEM models stored in the workbook (a custom XML part beside the
//! scripts part): saved, carried, replaced, removed, and the package stays
//! valid for Excel.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sheets_engine::Workbook;
use sheets_engine::pls_models::PlsModel;
use sheets_engine::scripts::{Script, ScriptLanguage};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pls_models");
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

fn custom_items(path: &Path) -> usize {
    parts(path)
        .keys()
        .filter(|k| k.starts_with("customXml/item") && !k.contains("Props"))
        .count()
}

const MODEL: &str = r#"{"constructs":[{"name":"X","indicators":["x1","x2"]},{"name":"Y","indicators":["y1"]}],"paths":[["X","Y"]],"layout":{"name":"Model 1","positions":{"X":[100.0,200.0]}}}"#;

#[test]
fn models_round_trip_beside_scripts() {
    let first = scratch("first.xlsx");
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "x1").unwrap();
    assert_eq!(wb.pls_models().unwrap(), vec![]);
    let models = vec![PlsModel::new("Model 1", MODEL)];
    wb.set_pls_models(models.clone());
    wb.set_scripts(vec![Script::new(
        "Tools",
        ScriptLanguage::JavaScript,
        "function go() {}\n",
    )]);
    assert!(wb.is_dirty());
    wb.save_as(&first).unwrap();
    assert_eq!(wb.save_notes(), &[] as &[String]);
    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    assert_eq!(custom_items(&first), 2);

    // Reopened, and carried by a save that didn't touch them.
    let mut wb = Workbook::open(&first).unwrap();
    assert_eq!(wb.pls_models().unwrap(), models);
    assert_eq!(wb.scripts().unwrap().len(), 1);
    wb.set_input(0, 1, 0, "x2").unwrap();
    let second = scratch("second.xlsx");
    wb.save_as(&second).unwrap();
    let wb = Workbook::open(&second).unwrap();
    assert_eq!(wb.pls_models().unwrap(), models);
    assert_eq!(custom_items(&second), 2);

    // Replaced, then removed; the scripts stay.
    let mut wb = Workbook::open(&second).unwrap();
    let changed = vec![
        PlsModel::new("Model 1", MODEL),
        PlsModel::new("Model 2", r#"{"constructs":[],"paths":[]}"#),
    ];
    wb.set_pls_models(changed.clone());
    let third = scratch("third.xlsx");
    wb.save_as(&third).unwrap();
    let wb = Workbook::open(&third).unwrap();
    assert_eq!(wb.pls_models().unwrap(), changed);
    assert_eq!(custom_items(&third), 2);
    let mut wb = Workbook::open(&third).unwrap();
    wb.set_pls_models(Vec::new());
    let fourth = scratch("fourth.xlsx");
    wb.save_as(&fourth).unwrap();
    let bytes = std::fs::read(&fourth).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    assert_eq!(custom_items(&fourth), 1);
    let wb = Workbook::open(&fourth).unwrap();
    assert_eq!(wb.pls_models().unwrap(), vec![]);
    assert_eq!(wb.scripts().unwrap().len(), 1);
}
