//! Round-trip fidelity over the `.xlsx` corpus (plan.md §0 item 9, §5, §7).
//!
//! For every file in `tests/corpus/xlsx/`:
//!
//! 1. **Round trip through Kantor**: open with [`sheets_engine::Workbook`],
//!    `save_as` to a temp file, reopen, and compare `input` and
//!    `display_value` of every used cell of every sheet.
//! 2. **Import losses**: read the original independently with calamine (cell
//!    values as cached by the producer, formulas, merged ranges, defined names,
//!    sheet visibility) and compare with what IronCalc loaded.
//! 3. **Export losses**: read the saved file with calamine too and compare it
//!    with the original (values, formulas, styles, merges, names, hyperlinks),
//!    plus layout (column widths, row heights, hidden, frozen panes), comments
//!    and links as IronCalc sees them after reopening.
//! 4. **Dropped parts**: zip entries of the original missing from the saved
//!    file, top-level elements of an original worksheet missing from the saved
//!    sheet (`dropped-element`), and package problems of the saved file
//!    (`package`: a part without a content type, a relationship whose target is
//!    missing; see [`sheets_engine::package_problems`]).
//! 5. **Expectations** written by the generator (`expectations.tsv`): what
//!    Excel displays for formatted cells, and the layout that was written.
//!
//! Every difference is a *finding*. Known IronCalc gaps are listed, with a
//! reason, in `tests/corpus/xlsx/known-gaps.tsv`. The test fails when a finding
//! is not in that list (a new regression) and when a listed gap no longer
//! happens (fixed: remove the line), so the list stays an honest record of
//! what Kantor still drops (§5 "never drop content").
//!
//! Environment:
//! - `KANTOR_CORPUS_BLESS=1` rewrites `known-gaps.tsv` from the current
//!   findings, keeping existing reasons (new lines get `TODO`).
//! - `KANTOR_CORPUS_OUT=<dir>` writes each round-tripped file as
//!   `<name>.roundtrip.xlsx` and a Markdown report `report.md` there (used by
//!   CI to render PDFs).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use calamine::{Data, Reader as _, Xlsx, open_workbook};
use ironcalc_base::Model;
use ironcalc_base::cell::CellValue;
use sheets_engine::{Workbook, cell_name, column_name};

const GAPS_FILE: &str = "known-gaps.tsv";

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx")
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    file: String,
    check: &'static str,
    location: String,
    detail: String,
}

impl Finding {
    fn key(&self) -> (String, String, String) {
        (
            self.file.clone(),
            self.check.to_owned(),
            self.location.clone(),
        )
    }
}

struct Findings {
    file: String,
    list: Vec<Finding>,
}

impl Findings {
    fn add(&mut self, check: &'static str, location: impl Into<String>, detail: impl Into<String>) {
        self.list.push(Finding {
            file: self.file.clone(),
            check,
            location: location.into(),
            detail: detail.into(),
        });
    }

    /// Record a difference between two values, if any.
    fn diff<T: PartialEq + std::fmt::Debug>(
        &mut self,
        check: &'static str,
        location: impl Into<String>,
        before: &T,
        after: &T,
    ) {
        if before != after {
            self.add(check, location, format!("{before:?} -> {after:?}"));
        }
    }
}

// ------------------------------------------------------------------ main test

#[test]
fn xlsx_corpus_fidelity() {
    let dir = corpus_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "xlsx"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 10,
        "expected the corpus in {}",
        dir.display()
    );

    let expectations = read_expectations(&dir.join("expectations.tsv"));
    let features = read_features(&dir.join("features.tsv"));
    let out_dir = std::env::var_os("KANTOR_CORPUS_OUT").map(PathBuf::from);
    if let Some(out) = &out_dir {
        fs::create_dir_all(out).unwrap();
    }
    let tmp = tempfile::tempdir().unwrap();

    let mut all = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let saved = tmp.path().join(&name);
        let mut f = Findings {
            file: name.clone(),
            list: Vec::new(),
        };
        check_file(path, &saved, &expectations, &features, &mut f);
        if let Some(out) = &out_dir
            && saved.exists()
        {
            let stem = name.trim_end_matches(".xlsx");
            fs::copy(&saved, out.join(format!("{stem}.roundtrip.xlsx"))).unwrap();
        }
        all.extend(f.list);
    }
    all.sort();
    all.dedup_by(|a, b| a.key() == b.key());

    let gaps_path = dir.join(GAPS_FILE);
    let gaps = read_gaps(&gaps_path);
    let (report, new, fixed) = compare(&all, &gaps, files.len());
    println!("{report}");
    if let Some(out) = &out_dir {
        fs::write(out.join("report.md"), &report).unwrap();
    }
    if std::env::var_os("KANTOR_CORPUS_BLESS").is_some() {
        write_gaps(&gaps_path, &all, &gaps);
        println!(
            "blessed {} findings into {}",
            all.len(),
            gaps_path.display()
        );
        return;
    }

    let mut msg = String::new();
    if !new.is_empty() {
        let _ = writeln!(
            msg,
            "{} NEW fidelity finding(s) (regressions, or new gaps to document in tests/corpus/xlsx/{GAPS_FILE}):",
            new.len()
        );
        for f in &new {
            let _ = writeln!(
                msg,
                "  {}\t{}\t{}\t{}",
                f.file, f.check, f.location, f.detail
            );
        }
    }
    if !fixed.is_empty() {
        let _ = writeln!(
            msg,
            "{} known gap(s) no longer happen; remove them from {GAPS_FILE}:",
            fixed.len()
        );
        for (file, check, loc) in &fixed {
            let _ = writeln!(msg, "  {file}\t{check}\t{loc}");
        }
    }
    assert!(msg.is_empty(), "{msg}");
}

fn check_file(
    path: &Path,
    saved: &Path,
    expectations: &[Expectation],
    features: &[FeatureProbe],
    f: &mut Findings,
) {
    // 1. Kantor round trip: open -> save_as -> reopen.
    let mut original = match Workbook::open(path) {
        Ok(wb) => wb,
        Err(e) => {
            f.add("open", "-", format!("{e:#}"));
            return;
        }
    };
    let snapshot = cells_of(&original);
    if let Err(e) = original.save_as(saved) {
        f.add("save", "-", format!("{e:#}"));
        return;
    }
    let mut reopened = match Workbook::open(saved) {
        Ok(wb) => wb,
        Err(e) => {
            f.add("reopen", "-", format!("{e:#}"));
            return;
        }
    };
    let after = cells_of(&reopened);
    f.diff(
        "roundtrip-sheets",
        "-",
        &original.sheet_names(),
        &reopened.sheet_names(),
    );
    for key in snapshot.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        let empty = (String::new(), String::new());
        let (a, b) = (
            snapshot.get(key).unwrap_or(&empty),
            after.get(key).unwrap_or(&empty),
        );
        f.diff("roundtrip-input", key.clone(), &a.0, &b.0);
        f.diff("roundtrip-display", key.clone(), &a.1, &b.1);
    }

    // IronCalc's models of both files, for checks the public API doesn't cover.
    let model_a = load_model(path);
    let model_b = load_model(saved);

    // 2 + 3. Independent reads of both files.
    let cal_a = CalamineView::read(path);
    let cal_b = CalamineView::read(saved);
    import_checks(&cal_a, &model_a, f);
    export_checks(&cal_a, &cal_b, f);
    layout_checks(&model_a, &model_b, f);

    // 4. Dropped package parts.
    let parts_a = zip_entries(path);
    let parts_b = zip_entries(saved);
    for part in parts_a.difference(&parts_b) {
        f.add("dropped-part", part.clone(), "zip entry missing after save");
    }
    for problem in sheets_engine::package_problems(&fs::read(saved).unwrap()) {
        f.add("package", problem, "saved package is invalid");
    }
    let (elems_a, elems_b) = (sheet_elements(path), sheet_elements(saved));
    for (sheet, ea) in &elems_a {
        let none = BTreeSet::new();
        let eb = elems_b.get(sheet).unwrap_or(&none);
        for e in ea.difference(eb) {
            f.add(
                "dropped-element",
                format!("{sheet} <{e}>"),
                "sheet element missing after save",
            );
        }
    }

    // 5. Generator expectations (what Excel shows / the layout written).
    let file = f.file.clone();
    for e in expectations.iter().filter(|e| e.file == file) {
        expectation_check(e, &original, &model_a, f);
    }

    // 6. Features (`features.tsv`): each probe matches in the original, after
    //    open -> save, and after saving the saved file again.
    let probes: Vec<FeatureProbe> = features.iter().filter(|p| p.file == file).cloned().collect();
    if probes.is_empty() {
        return;
    }
    let twice = saved.with_extension("twice.xlsx");
    let second = match reopened.save_as(&twice) {
        Ok(()) => Some(Parts::read(&twice)),
        Err(e) => {
            f.add("save", "second", format!("{e:#}"));
            None
        }
    };
    let (pa, pb) = (Parts::read(path), Parts::read(saved));
    for p in &probes {
        if let Err(e) = p.check(&pa) {
            f.add("feature-fixture", p.feature.clone(), e);
            continue;
        }
        if let Err(e) = p.check(&pb) {
            f.add("feature", p.feature.clone(), format!("lost on save: {e}"));
        } else if let Some(Err(e)) = second.as_ref().map(|s| p.check(s)) {
            f.add("feature", p.feature.clone(), format!("lost on second save: {e}"));
        }
    }
}

// ------------------------------------------------------------------ feature probes

/// A line of `features.tsv` (see `scripts/corpus/gen_features_xlsx.py`).
#[derive(Clone)]
struct FeatureProbe {
    file: String,
    feature: String,
    part: String,
    probe: String,
}

fn read_features(path: &Path) -> Vec<FeatureProbe> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let p: Vec<&str> = l.split('\t').collect();
            assert_eq!(p.len(), 4, "bad features line: {l}");
            FeatureProbe {
                file: p[0].into(),
                feature: p[1].into(),
                part: p[2].into(),
                probe: p[3].into(),
            }
        })
        .collect()
}

/// Every part of a package, by name.
struct Parts(BTreeMap<String, Vec<u8>>);

impl Parts {
    fn read(path: &Path) -> Self {
        let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut parts = BTreeMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            parts.insert(entry.name().unwrap().into_owned(), data);
        }
        Parts(parts)
    }

    fn text(&self, name: &str) -> Option<String> {
        self.0
            .get(name)
            .map(|d| String::from_utf8_lossy(d).into_owned())
    }

    /// `(kind, target part)` of the relationships of `source`.
    fn rels(&self, source: &str) -> Vec<(String, String)> {
        let (dir, file) = source.rsplit_once('/').unwrap_or(("", source));
        let rels = format!("{dir}/_rels/{file}.rels");
        let Some(xml) = self.text(&rels) else {
            return Vec::new();
        };
        let doc = roxmltree::Document::parse(&xml).unwrap();
        doc.descendants()
            .filter(|n| n.has_tag_name("Relationship") && n.attribute("TargetMode").is_none())
            .map(|n| {
                let kind = n.attribute("Type").unwrap().rsplit('/').next().unwrap();
                let target = n.attribute("Target").unwrap();
                (kind.to_owned(), resolve_target(dir, target))
            })
            .collect()
    }

    /// The worksheet or chart sheet part of the sheet called `name`.
    fn sheet_part(&self, name: &str) -> Option<String> {
        let xml = self.text("xl/workbook.xml")?;
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let sheet = doc
            .descendants()
            .find(|n| n.has_tag_name("sheet") && n.attribute("name") == Some(name))?;
        let id = sheet
            .attributes()
            .find(|a| a.name() == "id" && a.namespace().is_some())?
            .value()
            .to_owned();
        let rels_xml = self.text("xl/_rels/workbook.xml.rels")?;
        let rels = roxmltree::Document::parse(&rels_xml).unwrap();
        let target = rels
            .descendants()
            .find(|n| n.attribute("Id") == Some(id.as_str()))?
            .attribute("Target")?;
        Some(resolve_target("xl", target))
    }
}

fn resolve_target(dir: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_owned(),
        None => format!("{dir}/{target}"),
    };
    let mut segs: Vec<&str> = Vec::new();
    for s in joined.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    segs.join("/")
}

/// One step of a probe: an element name and conditions on it.
struct Step {
    /// Searched among all descendants (`//`), not only children.
    deep: bool,
    name: String,
    /// `(attribute, value)`; `None` asks only for the attribute; the attribute
    /// `text()` is the element's text.
    conds: Vec<(String, Option<String>)>,
}

fn parse_probe(probe: &str) -> Vec<Step> {
    // Split on `/` outside brackets.
    let mut raw: Vec<(bool, String)> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut deep = false;
    let mut chars = probe.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '[' => depth += 1,
            ']' => depth -= 1,
            '/' if depth == 0 => {
                raw.push((deep, std::mem::take(&mut cur)));
                deep = chars.peek() == Some(&'/');
                if deep {
                    chars.next();
                }
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    raw.push((deep, cur));
    raw.into_iter()
        .enumerate()
        .map(|(i, (deep, s))| {
            let (name, rest) = s.split_once('[').map_or((s.as_str(), ""), |(n, r)| (n, r));
            let conds = rest
                .trim_end_matches(']')
                .split("][")
                .filter(|c| !c.is_empty())
                .map(|c| {
                    let c = c.strip_prefix('@').unwrap_or(c);
                    match c.split_once('=') {
                        Some((k, v)) => (k.to_owned(), Some(v.to_owned())),
                        None => (c.to_owned(), None),
                    }
                })
                .collect();
            Step {
                deep: deep || i == 0,
                name: name.to_owned(),
                conds,
            }
        })
        .collect()
}

fn step_matches(n: roxmltree::Node, step: &Step) -> bool {
    n.is_element()
        && n.tag_name().name() == step.name
        && step.conds.iter().all(|(k, v)| {
            let got = if k == "text()" {
                Some(n.text().unwrap_or_default())
            } else {
                n.attributes().find(|a| a.name() == k).map(|a| a.value())
            };
            match (got, v) {
                (Some(g), Some(v)) => g == v,
                (Some(_), None) => true,
                (None, _) => false,
            }
        })
}

fn probe_matches(node: roxmltree::Node, steps: &[Step]) -> bool {
    let Some((step, rest)) = steps.split_first() else {
        return true;
    };
    let candidates: Box<dyn Iterator<Item = roxmltree::Node>> = if step.deep {
        Box::new(node.descendants().skip(1))
    } else {
        Box::new(node.children())
    };
    candidates
        .filter(|c| step_matches(*c, step))
        .any(|c| probe_matches(c, rest))
}

impl FeatureProbe {
    /// The parts the selector names.
    fn parts(&self, pkg: &Parts) -> Result<Vec<String>, String> {
        let sel = self.part.as_str();
        let parts = if let Some(name) = sel.strip_prefix("pkg:") {
            vec![name.to_owned()]
        } else if sel == "styles" {
            vec!["xl/styles.xml".to_owned()]
        } else if sel == "workbook" {
            vec!["xl/workbook.xml".to_owned()]
        } else if let Some(sheet) = sel.strip_prefix("sheet:") {
            vec![
                pkg.sheet_part(sheet)
                    .ok_or_else(|| format!("no sheet {sheet:?}"))?,
            ]
        } else if let Some(rest) = sel.strip_prefix("rel:") {
            let (sheet, kind) = rest.rsplit_once(':').expect("rel:<sheet>:<kind>");
            let part = pkg
                .sheet_part(sheet)
                .ok_or_else(|| format!("no sheet {sheet:?}"))?;
            pkg.rels(&part)
                .into_iter()
                .filter(|(k, _)| k == kind)
                .map(|(_, t)| t)
                .collect()
        } else {
            panic!("unknown part selector {sel}")
        };
        let parts: Vec<String> = parts.into_iter().filter(|p| pkg.0.contains_key(p)).collect();
        if parts.is_empty() {
            return Err(format!("no part {sel}"));
        }
        Ok(parts)
    }

    fn check(&self, pkg: &Parts) -> Result<(), String> {
        let parts = self.parts(pkg)?;
        if self.probe.is_empty() {
            return Ok(());
        }
        let steps = parse_probe(&self.probe);
        for part in &parts {
            let xml = pkg.text(part).unwrap_or_default();
            let doc = roxmltree::Document::parse(&xml).map_err(|e| format!("{part}: {e}"))?;
            if probe_matches(doc.root(), &steps) {
                return Ok(());
            }
        }
        Err(format!("{} not in {}", self.probe, parts.join(", ")))
    }
}

/// `"Sheet!A1" -> (input, display)` for every used cell.
fn cells_of(wb: &Workbook) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    for (i, sheet) in wb.sheet_names().iter().enumerate() {
        let i = i as u32;
        let (rows, cols) = wb.used_size(i);
        for r in 0..rows {
            for c in 0..cols {
                let input = wb.input(i, r, c);
                let display = wb.display_value(i, r, c);
                if !input.is_empty() || !display.is_empty() {
                    out.insert(format!("{sheet}!{}", cell_name(r, c)), (input, display));
                }
            }
        }
    }
    out
}

fn load_model(path: &Path) -> Model<'static> {
    let mut model = ironcalc::import::load_from_xlsx(path.to_str().unwrap(), "en", "UTC", "en")
        .unwrap_or_else(|e| panic!("IronCalc can't load {}: {e}", path.display()));
    model.evaluate();
    model
}

// ------------------------------------------------------------------ calamine view

/// A canonical value, comparable across readers.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Empty,
    Num(f64),
    Str(String),
    Bool(bool),
    Err(String),
}

impl Val {
    fn from_calamine(d: &Data) -> Self {
        match d {
            Data::Empty => Val::Empty,
            Data::Int(i) => Val::Num(*i as f64),
            Data::Float(x) => Val::Num(*x),
            Data::String(s) if s.is_empty() => Val::Empty,
            Data::String(s) => Val::Str(s.clone()),
            Data::Bool(b) => Val::Bool(*b),
            Data::DateTime(d) => Val::Num(d.as_f64()),
            Data::DateTimeIso(s) | Data::DurationIso(s) => Val::Str(s.clone()),
            Data::Error(e) => Val::Err(e.to_string()),
        }
    }

    fn from_ironcalc(v: CellValue) -> Self {
        match v {
            CellValue::None => Val::Empty,
            CellValue::String(s) if s.is_empty() => Val::Empty,
            CellValue::String(s) => Val::Str(s),
            CellValue::Number(x) => Val::Num(x),
            CellValue::Boolean(b) => Val::Bool(b),
        }
    }

    /// Equal up to float noise; an error equals the same text as a string
    /// (IronCalc reports errors as strings).
    fn same(&self, other: &Val) -> bool {
        match (self, other) {
            (Val::Num(a), Val::Num(b)) => {
                a == b || (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
            }
            (Val::Err(a), Val::Str(b)) | (Val::Str(b), Val::Err(a)) => a == b,
            (a, b) => a == b,
        }
    }
}

#[derive(Default)]
struct SheetView {
    visible: String,
    values: BTreeMap<(u32, u32), Val>,
    formulas: BTreeMap<(u32, u32), String>,
    /// `(row, col) -> [(property, debug value)]`, only for cells with a style.
    styles: BTreeMap<(u32, u32), Vec<(&'static str, String)>>,
    merges: BTreeSet<((u32, u32), (u32, u32))>,
    links: BTreeSet<String>,
    /// calamine could not read part of this sheet (`what: error`).
    errors: Vec<String>,
}

struct CalamineView {
    sheets: Vec<(String, SheetView)>,
    names: BTreeSet<(String, String)>,
}

impl CalamineView {
    fn read(path: &Path) -> Self {
        let mut wb: Xlsx<_> = open_workbook(path)
            .unwrap_or_else(|e| panic!("calamine can't open {}: {e}", path.display()));
        let meta: Vec<_> = wb.sheets_metadata().to_vec();
        let names = wb.defined_names().iter().cloned().collect();
        let mut sheets = Vec::new();
        for sheet in meta {
            // Chart sheets have no cells; the feature probes cover them.
            if sheet.typ == calamine::SheetType::ChartSheet {
                continue;
            }
            let name = sheet.name.clone();
            let mut v = SheetView {
                visible: format!("{:?}", sheet.visible),
                ..Default::default()
            };
            match wb.worksheet_range(&name) {
                Ok(range) => {
                    let (r0, c0) = range.start().unwrap_or((0, 0));
                    for (r, c, d) in range.used_cells() {
                        let val = Val::from_calamine(d);
                        if val != Val::Empty {
                            v.values.insert((r0 + r as u32, c0 + c as u32), val);
                        }
                    }
                }
                Err(e) => v.errors.push(format!("values: {e}")),
            }
            match wb.worksheet_formula(&name) {
                Ok(range) => {
                    let (r0, c0) = range.start().unwrap_or((0, 0));
                    for (r, c, formula) in range.used_cells() {
                        if !formula.is_empty() {
                            v.formulas
                                .insert((r0 + r as u32, c0 + c as u32), formula.clone());
                        }
                    }
                }
                Err(e) => v.errors.push(format!("formulas: {e}")),
            }
            match wb.worksheet_style(&name) {
                Ok(styles) => {
                    for (r, c, style) in styles.cells() {
                        let props = style_props(style);
                        if !props.is_empty() {
                            v.styles.insert((r, c), props);
                        }
                    }
                }
                Err(e) => v.errors.push(format!("styles: {e}")),
            }
            match wb.merge_cells_by_sheet_name(&name) {
                Ok(dims) => v.merges.extend(dims.into_iter().map(|d| (d.start, d.end))),
                Err(e) => v.errors.push(format!("merged cells: {e}")),
            }
            match wb.hyperlinks_by_sheet_name(&name) {
                Ok(links) => v.links.extend(links.iter().map(|l| format!("{l:?}"))),
                Err(e) => v.errors.push(format!("hyperlinks: {e}")),
            }
            sheets.push((name, v));
        }
        Self { sheets, names }
    }

    fn sheet(&self, name: &str) -> Option<&SheetView> {
        self.sheets.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }
}

/// Style properties that differ from a plain cell, as `(name, value)`.
fn style_props(style: &calamine::Style) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(font) = &style.font {
        let f = font;
        if f.is_bold() {
            out.push(("bold", "true".into()));
        }
        if f.is_italic() {
            out.push(("italic", "true".into()));
        }
        if f.has_underline() {
            out.push(("underline", format!("{:?}", f.underline)));
        }
        if f.has_strikethrough() {
            out.push(("strike", "true".into()));
        }
        if let Some(c) = &f.color {
            out.push(("font-color", format!("{:08X}", c.to_argb())));
        }
        if let Some(sz) = f.size {
            out.push(("font-size", format!("{sz}")));
        }
        if let Some(name) = &f.name {
            out.push(("font-name", name.clone()));
        }
    }
    if let Some(fill) = &style.fill
        && fill.is_visible()
    {
        out.push((
            "fill",
            format!(
                "{:?} {}",
                fill.pattern,
                fill.get_color()
                    .map(|c| format!("{:08X}", c.to_argb()))
                    .unwrap_or_default()
            ),
        ));
    }
    if let Some(b) = &style.borders {
        for (side, border) in [
            ("border-left", &b.left),
            ("border-right", &b.right),
            ("border-top", &b.top),
            ("border-bottom", &b.bottom),
        ] {
            if border.is_visible() {
                out.push((side, format!("{:?}", border.style)));
            }
        }
    }
    if let Some(a) = &style.alignment {
        let s = format!("{:?}/{:?}", a.horizontal, a.vertical);
        let default = format!(
            "{:?}/{:?}",
            calamine::HorizontalAlignment::default(),
            calamine::VerticalAlignment::default()
        );
        if s != default {
            out.push(("align", s));
        }
        if a.wrap_text {
            out.push(("wrap", "true".into()));
        }
    }
    if let Some(nf) = &style.number_format
        && !nf.format_code.eq_ignore_ascii_case("general")
        && !nf.format_code.is_empty()
    {
        out.push(("numfmt", nf.format_code.clone()));
    }
    out
}

fn a1(sheet: &str, (r, c): (u32, u32)) -> String {
    format!("{sheet}!{}", cell_name(r, c))
}

// ------------------------------------------------------------------ checks

/// What IronCalc loaded vs. what the original file holds.
fn import_checks(cal: &CalamineView, model: &Model, f: &mut Findings) {
    let names: Vec<String> = model
        .workbook
        .worksheets
        .iter()
        .map(|w| w.name.clone())
        .collect();
    let cal_names: Vec<String> = cal.sheets.iter().map(|(n, _)| n.clone()).collect();
    f.diff("import-sheets", "-", &cal_names, &names);

    for (idx, (sheet, view)) in cal.sheets.iter().enumerate() {
        for e in &view.errors {
            f.add("original-unreadable", sheet.clone(), e.clone());
        }
        let Some(ws) = model.workbook.worksheets.get(idx) else {
            continue;
        };
        // Both crates name the states Visible / Hidden / VeryHidden.
        let state = format!("{:?}", ws.state);
        f.diff("import-visibility", sheet.clone(), &view.visible, &state);

        // Values: every cell the original has (cached results for formulas).
        for (&pos, val) in &view.values {
            let ic = model
                .get_cell_value_by_index(idx as u32, pos.0 as i32 + 1, pos.1 as i32 + 1)
                .map(Val::from_ironcalc)
                .unwrap_or(Val::Empty);
            if !val.same(&ic) {
                f.add("import-value", a1(sheet, pos), format!("{val:?} -> {ic:?}"));
            }
        }
        // Formulas, as text.
        for (&pos, formula) in &view.formulas {
            let ic = model
                .get_cell_formula(idx as u32, pos.0 as i32 + 1, pos.1 as i32 + 1)
                .ok()
                .flatten()
                .unwrap_or_default();
            let ic = ic.strip_prefix('=').unwrap_or(&ic).to_owned();
            f.diff("import-formula", a1(sheet, pos), formula, &ic);
        }
        // Merged ranges.
        let ic_merges: BTreeSet<_> = ws
            .merged_cells
            .iter()
            .map(|m| {
                (
                    (m.row as u32 - 1, m.column as u32 - 1),
                    (
                        (m.row + m.height - 1) as u32 - 1,
                        (m.column + m.width - 1) as u32 - 1,
                    ),
                )
            })
            .collect();
        f.diff("import-merges", sheet.clone(), &view.merges, &ic_merges);
    }

    // Defined names (calamine drops the scope; compare name + formula).
    let ic: BTreeSet<(String, String)> = model
        .workbook
        .defined_names
        .iter()
        .map(|d| (d.name.clone(), d.formula.clone()))
        .collect();
    for (name, formula) in &cal.names {
        if !ic.contains(&(name.clone(), formula.clone())) {
            let got = ic.iter().find(|(n, _)| n == name).map(|(_, f)| f.clone());
            f.add(
                "import-name",
                name.clone(),
                format!("{formula:?} -> {got:?}"),
            );
        }
    }
}

/// The original vs. the file Kantor saved, both read by calamine.
fn export_checks(a: &CalamineView, b: &CalamineView, f: &mut Findings) {
    for (sheet, va) in &a.sheets {
        let Some(vb) = b.sheet(sheet) else {
            f.add("export-sheet", sheet.clone(), "sheet missing after save");
            continue;
        };
        f.diff("export-visibility", sheet.clone(), &va.visible, &vb.visible);
        if !vb.errors.is_empty() {
            // One finding instead of a cascade of "value missing" ones.
            f.add("export-unreadable", sheet.clone(), vb.errors.join("; "));
            continue;
        }
        let keys: BTreeSet<_> = va.values.keys().chain(vb.values.keys()).collect();
        for pos in keys {
            let x = va.values.get(pos).cloned().unwrap_or(Val::Empty);
            let y = vb.values.get(pos).cloned().unwrap_or(Val::Empty);
            if !x.same(&y) {
                f.add("export-value", a1(sheet, *pos), format!("{x:?} -> {y:?}"));
            }
        }
        let keys: BTreeSet<_> = va.formulas.keys().chain(vb.formulas.keys()).collect();
        for pos in keys {
            f.diff(
                "export-formula",
                a1(sheet, *pos),
                &va.formulas.get(pos),
                &vb.formulas.get(pos),
            );
        }
        let keys: BTreeSet<_> = va.styles.keys().chain(vb.styles.keys()).collect();
        let none = Vec::new();
        for pos in keys {
            let sa = va.styles.get(pos).unwrap_or(&none);
            let sb = vb.styles.get(pos).unwrap_or(&none);
            let props: BTreeSet<&str> = sa.iter().chain(sb.iter()).map(|(k, _)| *k).collect();
            for prop in props {
                let get = |s: &Vec<(&str, String)>| {
                    s.iter().find(|(k, _)| *k == prop).map(|(_, v)| v.clone())
                };
                f.diff(
                    "export-style",
                    format!("{} {prop}", a1(sheet, *pos)),
                    &get(sa),
                    &get(sb),
                );
            }
        }
        f.diff("export-merges", sheet.clone(), &va.merges, &vb.merges);
        f.diff("export-links", sheet.clone(), &va.links, &vb.links);
    }
    for (name, formula) in &a.names {
        let after = b.names.iter().find(|(n, _)| n == name).map(|(_, f)| f);
        f.diff("export-name", name.clone(), &Some(formula), &after);
    }
}

/// Layout, comments and links as IronCalc sees them, before and after saving.
fn layout_checks(a: &Model, b: &Model, f: &mut Findings) {
    for wa in &a.workbook.worksheets {
        let Some(wb) = b.workbook.worksheets.iter().find(|w| w.name == wa.name) else {
            continue;
        };
        let s = &wa.name;
        let cols = |w: &ironcalc_base::types::Worksheet| -> BTreeMap<String, (String, bool)> {
            let mut m = BTreeMap::new();
            for col in &w.cols {
                for c in col.min..=col.max.min(col.min + 255) {
                    let width = if col.custom_width {
                        format!("{:.2}", col.width)
                    } else {
                        "default".into()
                    };
                    m.insert(column_name(c as u32 - 1), (width, col.hidden));
                }
            }
            m
        };
        let (ca, cb) = (cols(wa), cols(wb));
        for key in ca.keys().chain(cb.keys()).collect::<BTreeSet<_>>() {
            f.diff(
                "export-column",
                format!("{s}!{key}"),
                &ca.get(key),
                &cb.get(key),
            );
        }
        let rows = |w: &ironcalc_base::types::Worksheet| -> BTreeMap<i32, (String, bool)> {
            w.rows
                .iter()
                .filter(|r| r.custom_height || r.hidden)
                .map(|r| (r.r, (format!("{:.2}", r.height), r.hidden)))
                .collect()
        };
        let (ra, rb) = (rows(wa), rows(wb));
        for key in ra.keys().chain(rb.keys()).collect::<BTreeSet<_>>() {
            f.diff(
                "export-row",
                format!("{s}!{key}"),
                &ra.get(key),
                &rb.get(key),
            );
        }
        f.diff(
            "export-frozen",
            s.clone(),
            &(wa.frozen_rows, wa.frozen_columns),
            &(wb.frozen_rows, wb.frozen_columns),
        );
        let comments = |w: &ironcalc_base::types::Worksheet| -> BTreeSet<String> {
            w.comments
                .iter()
                .map(|c| format!("{} {}: {}", c.cell_ref, c.author_name, c.text))
                .collect()
        };
        f.diff("export-comments", s.clone(), &comments(wa), &comments(wb));
        f.diff("export-tab-color", s.clone(), &wa.color, &wb.color);
        let links = |w: &ironcalc_base::types::Worksheet| -> BTreeSet<String> {
            w.links
                .iter()
                .map(|(k, v)| format!("{k:?} {v:?}"))
                .collect()
        };
        f.diff("export-links-model", s.clone(), &links(wa), &links(wb));
    }
}

fn zip_entries(path: &Path) -> BTreeSet<String> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let mut sink = Vec::new();
        // Reading proves the entry is intact, not just listed.
        entry.read_to_end(&mut sink).unwrap();
        out.insert(entry.name().unwrap().into_owned());
    }
    out
}

/// Sheet name -> local names of the top-level elements of its worksheet XML.
fn sheet_elements(path: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut read = |name: &str| -> String {
        let mut s = String::new();
        archive
            .by_name(name)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .read_to_string(&mut s)
            .unwrap();
        s
    };
    let rels_xml = read("xl/_rels/workbook.xml.rels");
    let rels = roxmltree::Document::parse(&rels_xml).unwrap();
    let target = |id: &str| -> String {
        let t = rels
            .descendants()
            .find(|n| n.attribute("Id") == Some(id))
            .and_then(|n| n.attribute("Target"))
            .unwrap();
        match t.strip_prefix('/') {
            Some(abs) => abs.to_owned(),
            None => format!("xl/{t}"),
        }
    };
    let wb_xml = read("xl/workbook.xml");
    let wb = roxmltree::Document::parse(&wb_xml).unwrap();
    let mut out = BTreeMap::new();
    for s in wb.descendants().filter(|n| n.has_tag_name("sheet")) {
        let id = s
            .attributes()
            .find(|a| a.name() == "id" && a.namespace().is_some())
            .unwrap()
            .value();
        let xml = read(&target(id));
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let names = doc
            .root_element()
            .children()
            .filter(|n| n.is_element())
            .map(|n| n.tag_name().name().to_owned())
            .collect();
        out.insert(s.attribute("name").unwrap().to_owned(), names);
    }
    out
}

// ------------------------------------------------------------------ expectations

struct Expectation {
    file: String,
    kind: String,
    sheet: String,
    place: String,
    expected: String,
}

fn read_expectations(path: &Path) -> Vec<Expectation> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let p: Vec<&str> = l.split('\t').collect();
            assert_eq!(p.len(), 5, "bad expectations line: {l}");
            Expectation {
                file: p[0].into(),
                kind: p[1].into(),
                sheet: p[2].into(),
                place: p[3].into(),
                expected: p[4].into(),
            }
        })
        .collect()
}

fn expectation_check(e: &Expectation, wb: &Workbook, model: &Model, f: &mut Findings) {
    let Some(idx) = wb.sheet_names().iter().position(|n| *n == e.sheet) else {
        f.add(
            "expect",
            format!("{}!{}", e.sheet, e.place),
            "sheet missing",
        );
        return;
    };
    let ws = &model.workbook.worksheets[idx];
    let loc = format!("{}!{} {}", e.sheet, e.place, e.kind);
    let col_of = |name: &str| -> i32 {
        name.bytes()
            .fold(0, |acc, b| acc * 26 + (b - b'A' + 1) as i32)
    };
    let got = match e.kind.as_str() {
        "display" => {
            let (r, c) = parse_a1(&e.place);
            wb.display_value(idx as u32, r, c)
        }
        "colwidth" => {
            let c = col_of(&e.place);
            ws.cols
                .iter()
                .find(|col| col.min <= c && c <= col.max && col.custom_width)
                .map(|col| format!("{}", col.width))
                .unwrap_or_else(|| "default".into())
        }
        "colhidden" => {
            let c = col_of(&e.place);
            let hidden = ws
                .cols
                .iter()
                .any(|col| col.min <= c && c <= col.max && col.hidden);
            if hidden { "1" } else { "0" }.into()
        }
        "rowheight" => {
            let r: i32 = e.place.parse().unwrap();
            ws.rows
                .iter()
                .find(|row| row.r == r && row.custom_height)
                .map(|row| format!("{}", row.height))
                .unwrap_or_else(|| "default".into())
        }
        "rowhidden" => {
            let r: i32 = e.place.parse().unwrap();
            let hidden = ws.rows.iter().any(|row| row.r == r && row.hidden);
            if hidden { "1" } else { "0" }.into()
        }
        "frozen" => format!("{},{}", ws.frozen_rows, ws.frozen_columns),
        other => panic!("unknown expectation kind {other}"),
    };
    let ok = match (got.parse::<f64>(), e.expected.parse::<f64>()) {
        (Ok(a), Ok(b)) if e.kind != "display" => (a - b).abs() < 1e-6,
        _ => got == e.expected,
    };
    if !ok {
        f.add("expect", loc, format!("{:?} -> {got:?}", e.expected));
    }
}

fn parse_a1(s: &str) -> (u32, u32) {
    let split = s.find(|ch: char| ch.is_ascii_digit()).unwrap();
    let (letters, digits) = s.split_at(split);
    let col = letters
        .bytes()
        .fold(0u32, |acc, b| acc * 26 + (b - b'A' + 1) as u32);
    (digits.parse::<u32>().unwrap() - 1, col - 1)
}

// ------------------------------------------------------------------ allowlist

type GapKey = (String, String, String);

/// `known-gaps.tsv`: `file  check  location  reason`. A location of `*` matches
/// every location of that check in that file.
fn read_gaps(path: &Path) -> BTreeMap<GapKey, String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let p: Vec<&str> = l.splitn(4, '\t').collect();
            assert!(p.len() >= 3, "bad {GAPS_FILE} line: {l}");
            (
                (p[0].into(), p[1].into(), p[2].into()),
                p.get(3).copied().unwrap_or("").into(),
            )
        })
        .collect()
}

fn gap_for<'a>(
    f: &Finding,
    gaps: &'a BTreeMap<GapKey, String>,
) -> Option<(&'a GapKey, &'a String)> {
    gaps.get_key_value(&f.key())
        .or_else(|| gaps.get_key_value(&(f.file.clone(), f.check.to_owned(), "*".to_owned())))
}

fn compare(
    all: &[Finding],
    gaps: &BTreeMap<GapKey, String>,
    file_count: usize,
) -> (String, Vec<Finding>, Vec<GapKey>) {
    let mut new = Vec::new();
    let mut used = BTreeSet::new();
    for f in all {
        match gap_for(f, gaps) {
            Some((key, _)) => {
                used.insert(key.clone());
            }
            None => new.push(f.clone()),
        }
    }
    let fixed: Vec<GapKey> = gaps
        .keys()
        .filter(|k| !used.contains(*k))
        .cloned()
        .collect();

    let mut r = String::new();
    let _ = writeln!(r, "# xlsx corpus fidelity report\n");
    let _ = writeln!(
        r,
        "{file_count} files, {} findings ({} known gaps, {} new), {} known gaps fixed.\n",
        all.len(),
        all.len() - new.len(),
        new.len(),
        fixed.len()
    );
    let mut by_check: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for f in all {
        *by_check.entry((&f.file, f.check)).or_default() += 1;
    }
    let _ = writeln!(r, "| File | Check | Findings |\n|---|---|---|");
    for ((file, check), n) in &by_check {
        let _ = writeln!(r, "| {file} | {check} | {n} |");
    }
    let parts: Vec<_> = all.iter().filter(|f| f.check == "dropped-part").collect();
    if !parts.is_empty() {
        let _ = writeln!(r, "\n## Warnings: package parts dropped on save\n");
        for f in parts {
            let _ = writeln!(r, "- WARNING {}: `{}` dropped", f.file, f.location);
        }
    }
    let _ = writeln!(r, "\n## All findings\n");
    for f in all {
        let mark = if gap_for(f, gaps).is_some() {
            "known"
        } else {
            "NEW"
        };
        let _ = writeln!(
            r,
            "- [{mark}] {} `{}` {}: {}",
            f.file, f.check, f.location, f.detail
        );
    }
    (r, new, fixed)
}

fn write_gaps(path: &Path, all: &[Finding], old: &BTreeMap<GapKey, String>) {
    let mut out = String::from(
        "# Known fidelity gaps in the xlsx corpus (IronCalc 0.9.1 import/export).\n\
         # Checked by crates/sheets_engine/tests/corpus.rs. Columns (tab-separated):\n\
         # file  check  location  reason. A location of * matches the whole check for that file.\n\
         # A finding not listed here fails the test; a listed gap that no longer happens fails too.\n",
    );
    // Keep wildcard lines that still match something.
    let mut written = BTreeSet::new();
    for f in all {
        let (key, reason) = match gap_for(f, old) {
            Some((k, reason)) => (k.clone(), reason.clone()),
            None => (f.key(), format!("TODO: {}", f.detail)),
        };
        if written.insert(key.clone()) {
            let _ = writeln!(out, "{}\t{}\t{}\t{}", key.0, key.1, key.2, reason);
        }
    }
    fs::write(path, out).unwrap();
}
