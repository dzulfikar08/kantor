//! Saving twice in one session: what Kantor added is not added again, and
//! an edit saved and then undone is undone in the next file too (whose
//! original is the first save).

use std::path::{Path, PathBuf};

use sheets_engine::{
    CellRange, CfCompare, CfFormat, CfRuleDef, CfSpec, ChartType, ErrorStyle, Message, Paper,
    Validation, ValidationOperator, ValidationType, Workbook,
};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("session_saves");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells = vec![(0, 0, "Name".to_owned()), (0, 1, "Value".to_owned())];
    for r in 1..6u32 {
        cells.push((r, 0, format!("N{r}")));
        cells.push((r, 1, (r * 10).to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    wb
}

fn count(path: &Path, needle: &str) -> usize {
    let p = sheets_engine_parts(path);
    p.values().map(|x| x.matches(needle).count()).sum()
}

fn sheets_engine_parts(path: &Path) -> std::collections::BTreeMap<String, String> {
    use std::io::Read as _;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.insert(e.name().unwrap().into_owned(), s);
        }
    }
    out
}

#[test]
fn saving_twice_adds_nothing_twice() {
    let mut wb = book();
    let data = CellRange::from_corners((0, 0), (5, 1));
    wb.create_table(0, data, true, "TableStyleMedium2").unwrap();
    wb.insert_chart(0, data, ChartType::Column).unwrap();
    let a = scratch("twice-a.xlsx");
    let b = scratch("twice-b.xlsx");
    wb.save_as(&a).unwrap();
    wb.save_as(&b).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert_eq!(count(&b, "<tablePart "), 1);
    assert_eq!(count(&b, "<c:chart "), 1);
    let back = Workbook::open(&b).unwrap();
    assert_eq!(back.tables(0).len(), 1);
    assert_eq!(back.drawings(0).len(), 1);
}

#[test]
fn an_edit_saved_then_undone_is_undone_in_the_next_save() {
    let mut wb = book();
    let data = CellRange::from_corners((0, 0), (5, 1));
    let mut p = wb.print_settings(0);
    p.setup.paper = Paper::LEGAL;
    wb.set_print_settings(0, p).unwrap();
    wb.add_cf_rule(
        0,
        "B2:B6",
        &CfRuleDef {
            spec: CfSpec::CellValue {
                op: CfCompare::GreaterThan,
                value: "20".into(),
                value2: String::new(),
            },
            format: CfFormat::PRESETS[0].1,
            stop_if_true: false,
        },
    )
    .unwrap();
    wb.set_validation(
        0,
        &[CellRange::from_corners((1, 1), (5, 1))],
        Validation {
            kind: ValidationType::Whole,
            operator: ValidationOperator::GreaterThan,
            allow_blank: true,
            dropdown: true,
            prompt: None,
            error: Some((ErrorStyle::Stop, Message::default())),
            formula1: Some("0".into()),
            formula2: None,
            ranges: Vec::new(),
        },
    )
    .unwrap();
    wb.create_table(0, data, true, "TableStyleMedium2").unwrap();
    wb.insert_chart(0, data, ChartType::Pie).unwrap();
    let first = scratch("undo-a.xlsx");
    wb.save_as(&first).unwrap();
    for _ in 0..5 {
        assert!(wb.undo().unwrap());
    }
    let second = scratch("undo-b.xlsx");
    wb.save_as(&second).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let mut back = Workbook::open(&second).unwrap();
    assert_eq!(back.print_settings(0).setup.paper, Paper::LETTER);
    assert!(back.cf_rules(0, None).is_empty());
    assert!(back.sheet_validations(0).is_empty());
    assert!(back.tables(0).is_empty());
    assert!(back.drawings(0).is_empty());
    assert!(back.validation(0, 2, 1).is_none());
    // Redo all and save again: everything is back.
    for _ in 0..5 {
        assert!(wb.redo().unwrap());
    }
    let third = scratch("undo-c.xlsx");
    wb.save_as(&third).unwrap();
    let back = Workbook::open(&third).unwrap();
    assert_eq!(back.print_settings(0).setup.paper, Paper::LEGAL);
    assert_eq!(back.cf_rules(0, None).len(), 1);
    assert_eq!(back.tables(0).len(), 1);
    assert_eq!(back.drawings(0).len(), 1);
}

/// Save in the background the way the app does: snapshot, edit meanwhile,
/// write on another thread, record.
fn save_in_background(wb: &mut Workbook, path: &Path, meanwhile: impl FnOnce(&mut Workbook)) {
    let job = wb.prepare_save(path).unwrap();
    let writer = std::thread::spawn(move || job.run());
    meanwhile(wb);
    let saved = writer.join().unwrap().unwrap();
    wb.finish_save(saved);
}

#[test]
fn background_saves_track_kantor_made_parts_too() {
    let mut wb = book();
    let data = CellRange::from_corners((0, 0), (5, 1));
    wb.create_table(0, data, true, "TableStyleMedium2").unwrap();
    wb.insert_chart(0, data, ChartType::Column).unwrap();
    let a = scratch("bg-a.xlsx");
    save_in_background(&mut wb, &a, |_| {});
    assert!(!wb.is_dirty());
    // Saved again over itself (the saved file is the original now).
    wb.set_input(0, 1, 1, "99").unwrap();
    save_in_background(&mut wb, &a, |_| {});
    assert_eq!(count(&a, "<tablePart "), 1);
    assert_eq!(count(&a, "<c:chart "), 1);
    // Undo the edit, save again: the file has the old value back.
    wb.undo().unwrap();
    assert!(wb.is_dirty());
    save_in_background(&mut wb, &a, |_| {});
    assert!(!wb.is_dirty());
    let back = Workbook::open(&a).unwrap();
    assert_eq!(back.display_value(0, 1, 1), "10");
    assert_eq!(back.tables(0).len(), 1);
    assert_eq!(back.drawings(0).len(), 1);
}

#[test]
fn edits_made_while_saving_stay_unsaved() {
    let mut wb = book();
    let path = scratch("bg-meanwhile.xlsx");
    wb.save_as(&path).unwrap();
    wb.set_input(0, 1, 1, "first").unwrap();
    save_in_background(&mut wb, &path, |wb| {
        wb.set_input(0, 2, 1, "second").unwrap();
        wb.set_frozen(0, 1, 0).unwrap();
    });
    assert!(wb.is_dirty(), "the edit made during the save isn't saved");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.display_value(0, 1, 1), "first");
    assert_eq!(back.display_value(0, 2, 1), "20");
    // Undo back to the saved state: the frozen pane (no undo step) came
    // after the snapshot, so it is still unsaved.
    wb.undo().unwrap();
    assert!(
        wb.is_dirty(),
        "frozen panes set during the save aren't saved"
    );
    wb.save().unwrap();
    assert!(!wb.is_dirty());
    assert_eq!(Workbook::open(&path).unwrap().frozen(0), (1, 0));
}

#[test]
fn autosave_then_save_adds_kantor_charts_once() {
    let mut wb = book();
    let data = CellRange::from_corners((0, 0), (5, 1));
    wb.insert_chart(0, data, ChartType::Column).unwrap();
    let path = scratch("autosave-chart.xlsx");
    wb.save_as(&path).unwrap();
    wb.set_input(0, 1, 1, "77").unwrap();
    let out = wb.xlsx_snapshot().render().unwrap();
    sheets_engine::write_file_atomic(&path, &out.bytes).unwrap();
    wb.commit_autosave(&path, out);
    wb.set_input(0, 1, 1, "78").unwrap();
    wb.save().unwrap();
    assert_eq!(count(&path, "<c:chart "), 1);
    assert_eq!(Workbook::open(&path).unwrap().drawings(0).len(), 1);
}

#[test]
fn a_failed_save_replaces_nothing_and_keeps_the_changes() {
    let mut wb = book();
    // Its own folder: the other tests save next to theirs meanwhile.
    let dir = scratch("failed-save");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fail.xlsx");
    wb.save_as(&path).unwrap();
    #[cfg(unix)]
    let before = std::fs::read(&path).unwrap();
    wb.set_input(0, 1, 1, "changed").unwrap();
    // A directory that doesn't exist (as when a disk is unplugged).
    let gone = scratch("no-such-dir").join("x.xlsx");
    let job = wb.prepare_save(&gone).unwrap();
    assert!(job.run().is_err());
    assert!(wb.is_dirty());
    assert_eq!(wb.path(), Some(path.as_path()));
    // A file without write permission is refused, not replaced.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let err = wb.save().unwrap_err();
        assert!(format!("{err:#}").contains("read-only"), "{err:#}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(wb.is_dirty());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        wb.save().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "the saved file keeps its permissions");
    }
    #[cfg(not(unix))]
    wb.save().unwrap();
    assert!(!wb.is_dirty());
    assert_eq!(
        Workbook::open(&path).unwrap().display_value(0, 1, 1),
        "changed"
    );
    let leftovers = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".kantor-tmp")
        })
        .count();
    assert_eq!(leftovers, 0);
}
