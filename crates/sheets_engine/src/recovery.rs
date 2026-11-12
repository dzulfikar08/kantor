//! Saving without stopping the UI: recovery copies and autosave.
//!
//! [`Workbook::xlsx_snapshot`] copies what an `.xlsx` save needs (IronCalc's cells,
//! the original package, pivot / drawing / tab state) on the UI thread; that
//! is a memory copy, far quicker than writing the file. [`XlsxSnapshot::render`]
//! then builds the `.xlsx` on any thread. An autosave hands the result back to
//! [`Workbook::commit_autosave`]; a recovery copy is just written somewhere
//! else, and [`Workbook::open_recovered`] reads it back as the original
//! document with unsaved changes.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use ironcalc_base::Model;

use crate::{
    CalcMode, LANGUAGE, Origin, SourceFormat, Workbook, XlsxParts, XlsxSource, any_unevaluated,
    build_xlsx, import, preserve, shift, write_atomic,
};

/// A workbook's state for writing an `.xlsx` off the UI thread (`Send`).
pub struct XlsxSnapshot {
    workbook: ironcalc_base::types::Workbook,
    original: Option<XlsxSource>,
    parts: XlsxParts,
    /// Manual calculation: formulas without a value ask Excel to recalculate.
    manual: bool,
    revision: u64,
    /// [`Revisions::view_changes`](crate::Revisions) when the snapshot was taken.
    view_changes: u64,
    layout: shift::Layout,
}

/// A rendered snapshot.
pub struct XlsxOutput {
    pub bytes: Vec<u8>,
    notes: Vec<String>,
    pivots: preserve::PivotsSaved,
    /// Where the charts made in Kantor went (the next save updates them there).
    charts: HashMap<u32, preserve::SavedChart>,
    revision: u64,
    view_changes: u64,
    layout: shift::Layout,
}

impl XlsxOutput {
    /// What the save couldn't carry over from the original (see
    /// [`Workbook::save_notes`]).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }
}

impl XlsxSnapshot {
    /// Build the `.xlsx` (slow for big workbooks: run it in the background).
    pub fn render(self) -> Result<XlsxOutput> {
        let full_calc = self.parts.full_calc || (self.manual && any_unevaluated(&self.workbook));
        let parts = XlsxParts {
            full_calc,
            ..self.parts
        };
        let model = Model::from_workbook(self.workbook, LANGUAGE)
            .map_err(|e| anyhow!("copying workbook: {e}"))?;
        let mut notes = Vec::new();
        let original = self.original.as_ref().and_then(|s| match s.bytes() {
            Ok(b) => Some(b),
            Err(e) => {
                notes.push(format!("extra parts of the original not kept: {e}"));
                None
            }
        });
        let finished = build_xlsx(&model, original.as_deref(), &parts)?;
        notes.extend(finished.notes);
        Ok(XlsxOutput {
            bytes: finished.bytes,
            notes,
            pivots: finished.pivots,
            charts: parts.patches.written.take().into_iter().collect(),
            revision: self.revision,
            view_changes: self.view_changes,
            layout: self.layout,
        })
    }
}

/// A save prepared by [`Workbook::prepare_save`] (`Send`).
pub struct SaveJob {
    path: PathBuf,
    kind: SaveKind,
}

enum SaveKind {
    Xlsx(Box<XlsxSnapshot>),
    Csv {
        workbook: Box<ironcalc_base::types::Workbook>,
        sheet: u32,
        delimiter: u8,
        revision: u64,
        view_changes: u64,
    },
}

/// A finished save, for [`Workbook::finish_save`].
pub struct Saved {
    path: PathBuf,
    kind: SavedKind,
}

enum SavedKind {
    Xlsx {
        out: Box<XlsxOutput>,
        /// The written file as the next save's original.
        xlsx: XlsxSource,
    },
    Csv {
        delimiter: u8,
        revision: u64,
        view_changes: u64,
    },
}

impl SaveJob {
    /// Where the save goes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Build the file and write it atomically (slow for big workbooks: run
    /// it in the background). On error nothing was replaced.
    pub fn run(self) -> Result<Saved> {
        let path = self.path;
        let kind = match self.kind {
            SaveKind::Xlsx(snapshot) => {
                let mut out = snapshot.render()?;
                write_file_atomic(&path, &out.bytes)?;
                let bytes = std::mem::take(&mut out.bytes);
                let xlsx = XlsxSource::kept(&path, bytes);
                SavedKind::Xlsx {
                    out: Box::new(out),
                    xlsx,
                }
            }
            SaveKind::Csv {
                workbook,
                sheet,
                delimiter,
                revision,
                view_changes,
            } => {
                let model = Model::from_workbook(*workbook, LANGUAGE)
                    .map_err(|e| anyhow!("copying workbook: {e}"))?;
                import::write_csv(&model, sheet, &path, delimiter)?;
                SavedKind::Csv {
                    delimiter,
                    revision,
                    view_changes,
                }
            }
        };
        Ok(Saved { path, kind })
    }
}

impl Saved {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the save couldn't carry over from the original.
    pub fn notes(&self) -> &[String] {
        match &self.kind {
            SavedKind::Xlsx { out, .. } => out.notes(),
            SavedKind::Csv { .. } => &[],
        }
    }
}

/// Write `bytes` to `path` atomically (temp file + rename).
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic(path, |file| Ok(file.write_all(bytes)?))
}

impl Workbook {
    /// Copy what saving needs, to [`XlsxSnapshot::render`] elsewhere.
    pub fn xlsx_snapshot(&self) -> XlsxSnapshot {
        XlsxSnapshot {
            workbook: self.model.get_model().workbook.clone(),
            original: self.origin.xlsx.clone(),
            parts: self.xlsx_parts(false),
            manual: self.calc.mode == CalcMode::Manual,
            revision: self.history.current,
            view_changes: self.history.view_changes,
            layout: self.history.layout(),
        }
    }

    /// Whether autosave may write this workbook to its file: it has one, and
    /// it's an `.xlsx` (a CSV keeps only one sheet's values; imports have no
    /// file to write).
    pub fn can_autosave(&self) -> bool {
        self.origin.format == SourceFormat::Xlsx
            && self.path.as_deref().is_some_and(|p| {
                matches!(
                    SourceFormat::from_extension(
                        p.extension().and_then(|e| e.to_str()).unwrap_or_default()
                    ),
                    Some(SourceFormat::Xlsx)
                )
            })
    }

    /// `out`, rendered from a [`Self::xlsx_snapshot`], was written to `path` (the
    /// workbook's file): record it as saved. Edits made since the snapshot
    /// stay unsaved.
    pub fn commit_autosave(&mut self, path: &Path, out: XlsxOutput) {
        let xlsx = XlsxSource::kept(path, out.bytes);
        self.commit_xlsx(path, xlsx, out.notes, out.pivots, out.charts, out.layout);
        self.saved_at(out.revision, out.view_changes);
    }

    /// The `.xlsx` written to `path` is the original of the next save.
    fn commit_xlsx(
        &mut self,
        path: &Path,
        xlsx: XlsxSource,
        notes: Vec<String>,
        pivots: preserve::PivotsSaved,
        charts: HashMap<u32, preserve::SavedChart>,
        layout: shift::Layout,
    ) {
        self.save_notes = notes;
        self.pivots_saved(&pivots);
        self.saved_charts = charts;
        self.path = Some(path.to_path_buf());
        self.origin = Origin {
            format: SourceFormat::Xlsx,
            path: Some(path.to_path_buf()),
            notes: Vec::new(),
            xlsx: Some(xlsx),
            xlsx_layout: layout,
        };
    }

    /// The state `revision` is what the file now holds. A view change
    /// (frozen panes) made after the snapshot keeps the workbook unsaved.
    fn saved_at(&mut self, revision: u64, view_changes: u64) {
        self.history.saved = revision;
        if self.history.view_changes == view_changes {
            self.history.view_changed = false;
        }
    }

    /// Prepare saving to `path` (see [`Workbook::save_as`] for formats): a
    /// copy of what the save needs, taken quickly here. [`SaveJob::run`]
    /// writes it on any thread, [`Workbook::finish_save`] records it.
    pub fn prepare_save(&self, path: impl AsRef<Path>) -> Result<SaveJob> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let kind = match SourceFormat::from_extension(ext) {
            Some(SourceFormat::Xlsx) => SaveKind::Xlsx(Box::new(self.xlsx_snapshot())),
            Some(SourceFormat::Csv { delimiter }) => {
                let delimiter = match self.origin.format {
                    SourceFormat::Csv { delimiter: d } if delimiter != b'\t' && d != b'\t' => d,
                    _ => delimiter,
                };
                SaveKind::Csv {
                    workbook: Box::new(self.model.get_model().workbook.clone()),
                    sheet: self.active_sheet(),
                    delimiter,
                    revision: self.history.current,
                    view_changes: self.history.view_changes,
                }
            }
            _ => bail!("Kantor saves .xlsx or .csv, not .{ext}"),
        };
        Ok(SaveJob {
            path: path.to_path_buf(),
            kind,
        })
    }

    /// Record a save [`SaveJob::run`] finished: the file is the workbook's
    /// (and the original of the next save). Edits made since
    /// [`Workbook::prepare_save`] stay unsaved.
    pub fn finish_save(&mut self, saved: Saved) {
        let path = saved.path;
        match saved.kind {
            SavedKind::Xlsx { out, xlsx } => {
                let out = *out;
                for note in &out.notes {
                    log::warn!("saving {}: {note}", path.display());
                }
                self.commit_xlsx(&path, xlsx, out.notes, out.pivots, out.charts, out.layout);
                self.saved_at(out.revision, out.view_changes);
            }
            SavedKind::Csv {
                delimiter,
                revision,
                view_changes,
            } => {
                self.save_notes = Vec::new();
                self.path = Some(path.clone());
                // A CSV save keeps the xlsx source: a later .xlsx save still
                // carries its parts, since the model still has every sheet.
                self.origin.format = SourceFormat::Csv { delimiter };
                self.origin.path = Some(path);
                self.origin.notes = Vec::new();
                self.saved_at(revision, view_changes);
            }
        }
    }

    /// Open a recovery copy as the document it was made from: `original`
    /// (opened as `format`, a [`SourceFormat::tag`]) with unsaved changes.
    /// Save writes to `original` only where it would have before (an `.xlsx`
    /// or a CSV); imports and new workbooks go to Save As.
    pub fn open_recovered(
        copy: &Path,
        original: Option<&Path>,
        format: Option<&str>,
    ) -> Result<Self> {
        let mut book = Self::open(copy)
            .with_context(|| format!("reading recovery copy {}", copy.display()))?;
        // The copy goes away once saved: keep its package in memory.
        if matches!(book.origin.xlsx, Some(XlsxSource::File { .. })) {
            book.origin.xlsx = Some(XlsxSource::Bytes(fs::read(copy)?));
        }
        let format = format
            .and_then(SourceFormat::from_tag)
            .or_else(|| {
                original.and_then(|p| {
                    SourceFormat::from_extension(p.extension()?.to_str().unwrap_or_default())
                })
            })
            .unwrap_or_default();
        match original {
            Some(orig) if format != SourceFormat::New => {
                book.origin.path = Some(orig.to_path_buf());
                book.origin.format = format;
                book.path = (!format.is_imported()).then(|| orig.to_path_buf());
            }
            _ => {
                book.origin.path = None;
                book.origin.format = SourceFormat::New;
                book.path = None;
            }
        }
        book.mark_modified();
        Ok(book)
    }

    /// Unsaved, whatever the undo history says (a restored copy).
    pub fn mark_modified(&mut self) {
        self.history.saved = u64::MAX;
    }

    /// The file a recovery copy should name as the original (see
    /// [`Self::source_path`]) and its format tag.
    pub fn recovery_origin(&self) -> (Option<PathBuf>, String) {
        (self.origin.path.clone(), self.origin.format.tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn corpus(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/xlsx")
            .join(name)
    }

    fn snapshot_to(book: &Workbook, path: &Path) {
        let out = std::thread::scope(|s| {
            let snap = book.xlsx_snapshot();
            s.spawn(move || snap.render()).join().unwrap()
        })
        .unwrap();
        write_file_atomic(path, &out.bytes).unwrap();
    }

    #[test]
    fn recovery_copy_round_trip_points_at_original() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("multi.xlsx");
        fs::copy(corpus("02_multi_sheet.xlsx"), &original).unwrap();
        let before = fs::read(&original).unwrap();

        let mut book = Workbook::open(&original).unwrap();
        book.set_input(0, 0, 0, "edited").unwrap();
        book.set_input(1, 2, 1, "=1+2").unwrap();
        let sheets = book.sheet_names();
        let copy = dir.path().join("copy-1.xlsx");
        snapshot_to(&book, &copy);
        // The snapshot doesn't save anything.
        assert!(book.is_dirty());
        assert_eq!(fs::read(&original).unwrap(), before);

        let (orig, tag) = book.recovery_origin();
        let back = Workbook::open_recovered(&copy, orig.as_deref(), Some(&tag)).unwrap();
        assert!(back.is_dirty(), "restored copies are unsaved");
        assert_eq!(back.path(), Some(original.as_path()));
        assert_eq!(back.source_path(), Some(original.as_path()));
        assert_eq!(back.title(), "multi.xlsx");
        assert_eq!(back.sheet_names(), sheets);
        assert_eq!(back.display_value(0, 0, 0), "edited");
        assert_eq!(back.display_value(1, 2, 1), "3");
        // Restoring didn't write the original either.
        assert_eq!(fs::read(&original).unwrap(), before);
    }

    #[test]
    fn recovered_imports_and_new_workbooks_never_save_over_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let ods = dir.path().join("simple.ods");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple.ods"),
            &ods,
        )
        .unwrap();
        let mut book = Workbook::open(&ods).unwrap();
        book.set_input(0, 0, 0, "x").unwrap();
        let copy = dir.path().join("copy.xlsx");
        snapshot_to(&book, &copy);
        let (orig, tag) = book.recovery_origin();
        assert_eq!(tag, "ods");
        let back = Workbook::open_recovered(&copy, orig.as_deref(), Some(&tag)).unwrap();
        assert_eq!(back.path(), None, "Save goes to Save As");
        assert_eq!(back.source_path(), Some(ods.as_path()));
        assert_eq!(back.suggested_file_name(), "simple.xlsx");
        assert!(!back.can_autosave());

        let mut new = Workbook::new_empty().unwrap();
        new.set_input(0, 0, 0, "draft").unwrap();
        snapshot_to(&new, &copy);
        let (orig, tag) = new.recovery_origin();
        let back = Workbook::open_recovered(&copy, orig.as_deref(), Some(&tag)).unwrap();
        assert_eq!(back.path(), None);
        assert_eq!(back.source_path(), None);
        assert_eq!(back.display_value(0, 0, 0), "draft");
        assert!(back.is_dirty());
    }

    #[test]
    fn recovered_csv_keeps_its_delimiter_but_never_autosaves() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("data.csv");
        fs::write(&csv, "a;b\n1;2\n").unwrap();
        let mut book = Workbook::open(&csv).unwrap();
        assert!(!book.can_autosave(), "CSV keeps one sheet's values only");
        book.set_input(0, 1, 0, "9").unwrap();
        let copy = dir.path().join("copy.xlsx");
        snapshot_to(&book, &copy);
        let (orig, tag) = book.recovery_origin();
        assert_eq!(tag, "csv:59");
        let mut back = Workbook::open_recovered(&copy, orig.as_deref(), Some(&tag)).unwrap();
        assert!(!back.can_autosave());
        back.save().unwrap();
        assert_eq!(fs::read_to_string(&csv).unwrap(), "a;b\r\n9;2\r\n");
    }

    #[test]
    fn autosave_commits_the_snapshot_state_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("charts.xlsx");
        fs::copy(corpus("14_charts_images.xlsx"), &path).unwrap();
        let mut book = Workbook::open(&path).unwrap();
        assert!(book.can_autosave());
        book.set_input(0, 0, 0, "first").unwrap();
        let snap = book.xlsx_snapshot();
        // Typed while the snapshot was being written.
        book.set_input(0, 200, 30, "second").unwrap();
        let out = snap.render().unwrap();
        write_file_atomic(&path, &out.bytes).unwrap();
        book.commit_autosave(&path, out);
        assert!(book.is_dirty(), "the later edit is still unsaved");
        book.undo().unwrap();
        assert!(
            !book.is_dirty(),
            "undo back to the autosaved state is clean"
        );

        let back = Workbook::open(&path).unwrap();
        assert_eq!(back.display_value(0, 0, 0), "first");
        assert_eq!(back.display_value(0, 200, 30), "");
        // The charts the original had are still in the file.
        let zip = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        assert!(
            zip.file_names()
                .any(|n| n.unwrap().starts_with("xl/charts/"))
        );
    }

    /// Timing on a big workbook: `KANTOR_BIG_XLSX=~/.cache/big90k.xlsx
    /// cargo test --release -p sheets_engine snapshot_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn snapshot_timing() {
        let Some(path) = std::env::var_os("KANTOR_BIG_XLSX") else {
            eprintln!("KANTOR_BIG_XLSX is not set");
            return;
        };
        let mut book = Workbook::open(&path).unwrap();
        book.set_input(0, 0, 0, "x").unwrap();
        for _ in 0..3 {
            let t = Instant::now();
            let snap = book.xlsx_snapshot();
            let ui = t.elapsed();
            let t = Instant::now();
            let out = snap.render().unwrap();
            let bg = t.elapsed();
            let t = Instant::now();
            write_file_atomic(&std::env::temp_dir().join("kantor-snap.xlsx"), &out.bytes).unwrap();
            eprintln!(
                "snapshot (UI thread) {ui:.2?}, render (background) {bg:.2?}, write {:.2?}, {} MB",
                t.elapsed(),
                out.bytes.len() / 1_000_000
            );
        }
        let snap = book.xlsx_snapshot();
        let t = Instant::now();
        let model = Model::from_workbook(snap.workbook, LANGUAGE).unwrap();
        eprintln!("  of which re-parsing formulas {:.2?}", t.elapsed());
        let t = Instant::now();
        let original = snap.original.as_ref().map(|s| s.bytes().unwrap());
        build_xlsx(&model, original.as_deref(), &snap.parts).unwrap();
        eprintln!("  of which writing the xlsx {:.2?}", t.elapsed());
        let t = Instant::now();
        book.save_as(std::env::temp_dir().join("kantor-save.xlsx"))
            .unwrap();
        eprintln!("a normal Save takes {:.2?} (UI thread)", t.elapsed());
        let t = Instant::now();
        let copy = std::env::temp_dir().join("kantor-snap.xlsx");
        let back = Workbook::open_recovered(&copy, Some(Path::new(&path)), Some("xlsx")).unwrap();
        eprintln!("open_recovered {:.2?}", t.elapsed());
        assert_eq!(back.display_value(0, 0, 0), "x");
    }
}
