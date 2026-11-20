//! Batch actions: the same steps on many PDFs (Acrobat's Action Wizard,
//! kept simple). A [`Batch`] is a list of [`Step`]s and where the results
//! go; it is saved and loaded as JSON (the PDF window's Batch dialog and
//! `kantor pdf batch steps.json` read the same files).
//!
//! Steps run in order on each file: place an image stamp or a Fill & Sign
//! signature picture, fill form fields (from a table, one row per file),
//! add page numbers, a watermark, flatten annotations. Then [`Plugin`]
//! steps run on the finished file's bytes, for steps that live outside
//! this crate (such as signing with a certificate): a step whose `type`
//! isn't built in is handed to the plugin registered for it.
//!
//! Digitally signed files stay valid: the changes are saved as an
//! incremental update (the signed bytes stay an exact prefix), steps the
//! certification (DocMDP) refuses are skipped, and steps that change the
//! page content (page numbers, watermark, flatten) are skipped on signed
//! files, each with a [`Reason`]. Password-protected files stay protected.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::annot::{self, Annot, Art, Kind};
use crate::form::{FieldKind, FieldValue};
use crate::pages::{page_ids, rotation, visible_box};
use crate::table::{Table, Values};
use crate::{Error, PageNumberOptions, PageRanges, Pdf, Position, Result, WatermarkOptions};

/// The batch file format's version.
pub const VERSION: u32 = 1;

/// Where `my-stamps/NAME` image references point (the PDF window's My
/// Stamps folder).
pub const MY_STAMPS: &str = "my-stamps/";

/// A batch: steps, in order, and where the results go.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    #[serde(default = "version")]
    pub version: u32,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub output: Output,
}

fn version() -> u32 {
    VERSION
}

impl Default for Batch {
    fn default() -> Self {
        Batch {
            version: VERSION,
            steps: Vec::new(),
            output: Output::default(),
        }
    }
}

impl Batch {
    pub fn from_json(text: &str) -> Result<Batch> {
        let b: Batch = serde_json::from_str(text)
            .map_err(|e| Error::Invalid(format!("not a Kantor batch file ({e})")))?;
        if b.version > VERSION {
            return Err(Error::Invalid(format!(
                "this batch file is from a newer Kantor (version {})",
                b.version
            )));
        }
        Ok(b)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Where the results go: next to each original with `suffix` added to its
/// name, or in `folder` (with the suffix too, when one is set).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    pub suffix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
}

impl Default for Output {
    fn default() -> Self {
        Output {
            suffix: "-batch".into(),
            folder: None,
        }
    }
}

impl Output {
    /// The result's path for `input`.
    pub fn path_for(&self, input: &Path) -> PathBuf {
        let stem = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".into());
        let name = format!("{stem}{}.pdf", self.suffix);
        match &self.folder {
            Some(f) => f.join(name),
            None => input.with_file_name(name),
        }
    }

    /// The results' paths for `inputs`, made unique within the batch (two
    /// files of the same name from different folders into one folder get
    /// `name (2).pdf`).
    pub fn paths_for(&self, inputs: &[PathBuf]) -> Vec<PathBuf> {
        let mut taken = HashSet::new();
        inputs
            .iter()
            .map(|i| {
                let first = self.path_for(i);
                let mut p = first.clone();
                let mut k = 2;
                while !taken.insert(p.clone()) {
                    let stem = first
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    p = first.with_file_name(format!("{stem} ({k}).pdf"));
                    k += 1;
                }
                p
            })
            .collect()
    }
}

/// One step of a batch.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// An image stamp (a company stamp PNG).
    Stamp(Stamp),
    /// A Fill & Sign signature picture (a picture of a handwritten
    /// signature; not a certificate signature).
    Signature(Stamp),
    Fill(Fill),
    PageNumbers(PageNumbers),
    Watermark(Watermark),
    /// Draw annotations (comments, stamps, signatures pictures, drawings)
    /// into the pages; links and form fields stay.
    Flatten,
    /// A step handled by a [`Plugin`] (its `type` and the rest of its JSON).
    Plugin {
        kind: String,
        options: serde_json::Value,
    },
}

impl Step {
    /// The step's `type` in the JSON.
    pub fn kind(&self) -> &str {
        match self {
            Step::Stamp(_) => "stamp",
            Step::Signature(_) => "signature",
            Step::Fill(_) => "fill",
            Step::PageNumbers(_) => "page-numbers",
            Step::Watermark(_) => "watermark",
            Step::Flatten => "flatten",
            Step::Plugin { kind, .. } => kind,
        }
    }
}

impl Serialize for Step {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let body = match self {
            Step::Stamp(x) | Step::Signature(x) => serde_json::to_value(x),
            Step::Fill(x) => serde_json::to_value(x),
            Step::PageNumbers(x) => serde_json::to_value(x),
            Step::Watermark(x) => serde_json::to_value(x),
            Step::Flatten => Ok(serde_json::Value::Object(Default::default())),
            Step::Plugin { options, .. } => Ok(options.clone()),
        }
        .map_err(S::Error::custom)?;
        let mut map = serde_json::Map::new();
        map.insert("type".into(), self.kind().into());
        if let serde_json::Value::Object(o) = body {
            for (k, v) in o {
                if k != "type" {
                    map.insert(k, v);
                }
            }
        }
        serde_json::Value::Object(map).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Step, D::Error> {
        use serde::de::Error as _;
        let mut v = serde_json::Value::deserialize(d)?;
        let kind = v
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| D::Error::custom("each step needs a \"type\""))?
            .to_owned();
        if let Some(o) = v.as_object_mut() {
            o.remove("type");
        }
        let parse = |v: serde_json::Value| -> std::result::Result<Step, serde_json::Error> {
            Ok(match kind.as_str() {
                "stamp" => Step::Stamp(serde_json::from_value(v)?),
                "signature" => Step::Signature(serde_json::from_value(v)?),
                "fill" => Step::Fill(serde_json::from_value(v)?),
                "page-numbers" => Step::PageNumbers(serde_json::from_value(v)?),
                "watermark" => Step::Watermark(serde_json::from_value(v)?),
                "flatten" => Step::Flatten,
                _ => Step::Plugin {
                    kind: kind.clone(),
                    options: v,
                },
            })
        };
        parse(v).map_err(|e| D::Error::custom(format!("step \"{kind}\": {e}")))
    }
}

/// Which pages: `all`, `first`, `last` or a range like `1-3,5`.
pub type Pages = String;

fn all_pages() -> Pages {
    "all".into()
}

/// A picture placed on pages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stamp {
    /// A PNG or JPEG file (relative paths are from the batch file's
    /// folder), or `my-stamps/NAME` for one of My Stamps (also `.ink`
    /// drawn signatures).
    pub image: String,
    pub pages: Pages,
    pub place: Place,
    /// Width in points; 0: the picture's own size (at most half the page).
    pub width: f32,
    /// 0..1.
    pub opacity: f32,
}

impl Default for Stamp {
    fn default() -> Self {
        Stamp {
            image: String::new(),
            pages: all_pages(),
            place: Place::default(),
            width: 0.0,
            opacity: 1.0,
        }
    }
}

/// Where a picture goes, in the page as displayed (its rotation applied),
/// in points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Place {
    /// At a corner, an edge's middle or the centre, moved `dx` / `dy`
    /// points in from the edges (for the middle positions: right / down).
    Anchor {
        anchor: Anchor,
        #[serde(default)]
        dx: f32,
        #[serde(default)]
        dy: f32,
    },
    /// The picture's top-left corner, measured from the page's top-left.
    Point { x: f32, y: f32 },
    /// The same spot as a picture placed on an example page: it keeps its
    /// distance to the nearest left/right and top/bottom edges on pages of
    /// other sizes, and its width.
    Example { example: Example },
}

impl Default for Place {
    fn default() -> Self {
        Place::Anchor {
            anchor: Anchor::BottomRight,
            dx: 36.0,
            dy: 36.0,
        }
    }
}

/// A picture placed on an example page (all in points, from the top-left
/// of the page as displayed).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub page_width: f32,
    pub page_height: f32,
}

/// Nine places on a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::TopCenter,
        Anchor::TopRight,
        Anchor::CenterLeft,
        Anchor::Center,
        Anchor::CenterRight,
        Anchor::BottomLeft,
        Anchor::BottomCenter,
        Anchor::BottomRight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Anchor::TopLeft => "top-left",
            Anchor::TopCenter => "top-center",
            Anchor::TopRight => "top-right",
            Anchor::CenterLeft => "center-left",
            Anchor::Center => "center",
            Anchor::CenterRight => "center-right",
            Anchor::BottomLeft => "bottom-left",
            Anchor::BottomCenter => "bottom-center",
            Anchor::BottomRight => "bottom-right",
        }
    }

    /// Column (0 left, 1 middle, 2 right) and row (0 top … 2 bottom).
    fn grid(self) -> (u8, u8) {
        let i = Anchor::ALL.iter().position(|a| *a == self).unwrap_or(0) as u8;
        (i % 3, i / 3)
    }
}

impl std::str::FromStr for Anchor {
    type Err = Error;
    fn from_str(s: &str) -> Result<Anchor> {
        let key: String = s
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .collect::<String>()
            .to_ascii_lowercase();
        let key = match key.as_str() {
            "top" => "topcenter",
            "bottom" => "bottomcenter",
            "left" => "centerleft",
            "right" => "centerright",
            "middle" => "center",
            k => k,
        };
        Anchor::ALL
            .into_iter()
            .find(|a| a.name().replace('-', "") == key)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "unknown place \"{s}\" (use top-left, top-center, top-right, center-left, \
                     center, center-right, bottom-left, bottom-center, bottom-right or X,Y)"
                ))
            })
    }
}

/// Fill form fields: `values` for every file, and a table's row per file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fill {
    /// A CSV, JSON or spreadsheet file (relative paths are from the batch
    /// file's folder): one row per file, matched by file name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The table's file name column (default: `file`, `filename`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The same values for every file (a table row wins).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: Values,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageNumbers {
    /// `{n}` the number, `{total}` the last number.
    pub format: String,
    pub position: String,
    pub start: i64,
    pub pages: Pages,
    pub size: f32,
    pub margin: f32,
}

impl Default for PageNumbers {
    fn default() -> Self {
        let d = PageNumberOptions::default();
        PageNumbers {
            format: d.format,
            position: d.position.name().into(),
            start: d.start,
            pages: all_pages(),
            size: d.font_size,
            margin: d.margin,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub text: String,
    pub size: f32,
    pub opacity: f32,
    /// Degrees counter-clockwise.
    pub rotation: f32,
    pub position: String,
    /// `#rrggbb`.
    pub color: String,
    pub pages: Pages,
}

impl Default for Watermark {
    fn default() -> Self {
        let d = WatermarkOptions::default();
        Watermark {
            text: String::new(),
            size: d.font_size,
            opacity: d.opacity,
            rotation: d.rotation,
            position: d.position.name().into(),
            color: "#808080".into(),
            pages: all_pages(),
        }
    }
}

// ── plugins ────────────────────────────────────────────────────────────────

/// What a plugin step knows about the file it works on.
pub struct FileContext<'a> {
    /// The input file's name.
    pub name: &'a str,
    /// The password that opened it (`Some("")` for files that open
    /// without one but are encrypted).
    pub password: Option<&'a str>,
    /// The file is digitally signed (before this batch).
    pub signed: bool,
}

/// A step that lives outside this crate (signing with a certificate…).
/// It runs on the finished file's bytes after every built-in step, so a
/// signature covers everything the batch did.
pub trait Plugin: Send + Sync {
    /// The step's `type` in the batch JSON.
    fn kind(&self) -> &str;
    /// Check a step's options when the batch is prepared.
    fn check(&self, _options: &serde_json::Value) -> std::result::Result<(), String> {
        Ok(())
    }
    /// The step must be the batch's last (and only one of its kind): a
    /// digital signature has to cover everything the batch did.
    fn last(&self) -> bool {
        false
    }
    /// Change the file: its new bytes, or skip the step for this file
    /// (and why), or an error to fail the file.
    fn apply(
        &self,
        options: &serde_json::Value,
        file: &FileContext,
        bytes: &[u8],
    ) -> std::result::Result<Applied, String>;
}

/// What a [`Plugin`] step did to a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    /// The file's new bytes.
    Changed(Vec<u8>),
    /// The step doesn't apply to this file: why (shown in the results).
    Skipped(String),
}

/// The plugins a batch may use.
#[derive(Clone, Default)]
pub struct Plugins(Vec<Arc<dyn Plugin>>);

impl Plugins {
    pub fn add(&mut self, p: Arc<dyn Plugin>) {
        self.0.push(p);
    }

    fn get(&self, kind: &str) -> Option<&Arc<dyn Plugin>> {
        self.0.iter().find(|p| p.kind() == kind)
    }
}

// ── preparing ──────────────────────────────────────────────────────────────

/// A picture ready to place.
#[derive(Clone, Debug)]
enum Picture {
    Image(Arc<Vec<u8>>),
    /// Drawn strokes, 0..1 in the box (y up).
    Ink(Arc<Vec<Vec<[f32; 2]>>>),
}

#[derive(Clone)]
enum Ready {
    Stamp {
        picture: Picture,
        /// Its own size in points.
        natural: (f32, f32),
        pages: Pages,
        place: Place,
        width: f32,
        opacity: f32,
        signature: bool,
    },
    Fill {
        table: Option<Arc<Table>>,
        values: Values,
    },
    PageNumbers(PageNumberOptions, Pages),
    Watermark(WatermarkOptions, Pages),
    Flatten,
    Plugin(Arc<dyn Plugin>, serde_json::Value),
}

/// A batch with its pictures and tables read, ready to run on files.
#[derive(Clone)]
pub struct Prepared {
    pub batch: Batch,
    steps: Vec<Ready>,
}

/// Read a stamp's picture: a PNG/JPEG file or a drawn `.ink` signature.
fn load_picture(path: &Path) -> Result<(Picture, (f32, f32))> {
    let bytes = crate::read(path)?;
    if path.extension().is_some_and(|e| e == "ink") {
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines();
        let aspect: f32 = lines
            .next()
            .and_then(|l| l.trim().parse().ok())
            .filter(|a: &f32| *a > 0.0)
            .ok_or_else(|| Error::Invalid(format!("{}: not a drawn signature", path.display())))?;
        let strokes: Vec<Vec<[f32; 2]>> = lines
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                l.split_whitespace()
                    .filter_map(|p| {
                        let (x, y) = p.split_once(',')?;
                        Some([x.parse().ok()?, y.parse().ok()?])
                    })
                    .collect()
            })
            .collect();
        return Ok((Picture::Ink(Arc::new(strokes)), (150.0, 150.0 / aspect)));
    }
    match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        _ => {
            return Err(Error::Invalid(format!(
                "{}: only PNG and JPEG pictures can be placed",
                path.display()
            )));
        }
    }
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .ok_or_else(|| Error::Invalid(format!("{}: can't read the picture", path.display())))?;
    Ok((
        Picture::Image(Arc::new(bytes)),
        (w.max(1) as f32 * 0.75, h.max(1) as f32 * 0.75),
    ))
}

/// A path written in a batch file: relative to `base`, or one of My Stamps.
pub fn resolve_path(spec: &str, base: &Path, my_stamps: Option<&Path>) -> PathBuf {
    if let Some(name) = spec.strip_prefix(MY_STAMPS)
        && let Some(dir) = my_stamps
    {
        return dir.join(name);
    }
    let p = Path::new(spec);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

fn check_pages(spec: &str, step: usize) -> Result<()> {
    page_spec(spec)
        .map(|_| ())
        .map_err(|e| Error::Invalid(format!("step {step}: {e}")))
}

fn page_spec(spec: &str) -> std::result::Result<PageRanges, crate::RangeError> {
    let s = spec.trim();
    if s.is_empty() {
        return Ok(PageRanges::all());
    }
    let s: Vec<String> = s
        .to_ascii_lowercase()
        .split(',')
        .map(|p| match p.trim() {
            "first" => "1".to_owned(),
            p => p.to_owned(),
        })
        .collect();
    PageRanges::parse(&s.join(","))
}

fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    let rgb = match h.len() {
        3 => [(v >> 8) & 15, (v >> 4) & 15, v & 15].map(|c| c * 17),
        6 => [(v >> 16) & 255, (v >> 8) & 255, v & 255],
        _ => return None,
    };
    Some(rgb.map(|c| c as f32 / 255.0))
}

impl Prepared {
    /// Read every picture and table the batch names (relative paths from
    /// `base`, `my-stamps/…` from `my_stamps`) and check its steps.
    pub fn new(
        batch: &Batch,
        base: &Path,
        my_stamps: Option<&Path>,
        plugins: &Plugins,
    ) -> Result<Prepared> {
        if batch.steps.is_empty() {
            return Err(Error::Invalid("the batch has no steps".into()));
        }
        let mut steps = Vec::new();
        let mut plugin_seen = false;
        for (i, s) in batch.steps.iter().enumerate() {
            let n = i + 1;
            let ctx = |e: Error| Error::Invalid(format!("step {n} ({}): {e}", s.kind()));
            if plugin_seen && !matches!(s, Step::Plugin { .. }) {
                return Err(Error::Invalid(format!(
                    "step {n} ({}) must come before the \"{}\" step",
                    s.kind(),
                    batch.steps[..i]
                        .iter()
                        .rev()
                        .find(|s| matches!(s, Step::Plugin { .. }))
                        .map_or("", Step::kind)
                )));
            }
            let ready = match s {
                Step::Stamp(st) | Step::Signature(st) => {
                    if st.image.trim().is_empty() {
                        return Err(ctx(Error::Invalid("choose a picture".into())));
                    }
                    check_pages(&st.pages, n)?;
                    let (picture, natural) =
                        load_picture(&resolve_path(&st.image, base, my_stamps)).map_err(ctx)?;
                    Ready::Stamp {
                        picture,
                        natural,
                        pages: st.pages.clone(),
                        place: st.place,
                        width: st.width.max(0.0),
                        opacity: st.opacity.clamp(0.05, 1.0),
                        signature: matches!(s, Step::Signature(_)),
                    }
                }
                Step::Fill(f) => {
                    let table = match &f.data {
                        Some(d) if !d.trim().is_empty() => {
                            let path = resolve_path(d, base, my_stamps);
                            let t = match &f.key {
                                Some(k) => {
                                    let ext = path
                                        .extension()
                                        .map(|e| e.to_string_lossy().to_ascii_lowercase());
                                    match ext.as_deref() {
                                        Some("csv" | "txt" | "tsv") => {
                                            let text = String::from_utf8_lossy(
                                                &crate::read(&path).map_err(ctx)?,
                                            )
                                            .into_owned();
                                            Table::from_csv(&text, Some(k))
                                        }
                                        Some("json") => Table::read(&path),
                                        _ => Table::from_sheet(&path, Some(k)),
                                    }
                                }
                                None => Table::read(&path),
                            }
                            .map_err(ctx)?;
                            Some(Arc::new(t))
                        }
                        _ => None,
                    };
                    if table.is_none() && f.values.is_empty() {
                        return Err(ctx(Error::Invalid(
                            "give the values or a table of values".into(),
                        )));
                    }
                    Ready::Fill {
                        table,
                        values: f.values.clone(),
                    }
                }
                Step::PageNumbers(p) => {
                    check_pages(&p.pages, n)?;
                    let position = p.position.parse::<Position>().map_err(ctx)?;
                    Ready::PageNumbers(
                        PageNumberOptions {
                            format: if p.format.is_empty() {
                                "{n}".into()
                            } else {
                                p.format.clone()
                            },
                            position,
                            start: p.start,
                            font_size: p.size.clamp(4.0, 200.0),
                            margin: p.margin.max(0.0),
                            ..PageNumberOptions::default()
                        },
                        p.pages.clone(),
                    )
                }
                Step::Watermark(w) => {
                    check_pages(&w.pages, n)?;
                    if w.text.trim().is_empty() {
                        return Err(ctx(Error::Invalid("type the watermark's text".into())));
                    }
                    let position = w.position.parse::<Position>().map_err(ctx)?;
                    let color = parse_hex(&w.color).ok_or_else(|| {
                        ctx(Error::Invalid(format!(
                            "\"{}\" is not a colour like #808080",
                            w.color
                        )))
                    })?;
                    Ready::Watermark(
                        WatermarkOptions {
                            text: w.text.clone(),
                            font_size: w.size.clamp(4.0, 400.0),
                            opacity: w.opacity.clamp(0.0, 1.0),
                            rotation: w.rotation,
                            position,
                            color,
                            ..WatermarkOptions::default()
                        },
                        w.pages.clone(),
                    )
                }
                Step::Flatten => Ready::Flatten,
                Step::Plugin { kind, options } => {
                    let p = plugins.get(kind).ok_or_else(|| {
                        Error::Invalid(format!(
                            "step {n}: this Kantor doesn't know the step \"{kind}\""
                        ))
                    })?;
                    p.check(options).map_err(|e| ctx(Error::Invalid(e)))?;
                    if p.last() && n != batch.steps.len() {
                        return Err(Error::Invalid(format!(
                            "step {n} ({kind}) must be the last step: it has to cover \
                             everything the batch did"
                        )));
                    }
                    plugin_seen = true;
                    Ready::Plugin(p.clone(), options.clone())
                }
            };
            steps.push(ready);
        }
        Ok(Prepared {
            batch: batch.clone(),
            steps,
        })
    }
}

// ── running ────────────────────────────────────────────────────────────────

/// Why a step (or a whole file) was skipped, or something to know about it.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// The file is password-protected and the password doesn't open it.
    Password,
    /// The step changes the page content: it would invalidate the digital
    /// signature.
    Signed,
    /// The certification allows no changes at all.
    NoChangesAllowed,
    /// The certification allows only form filling and signing.
    FormFillOnly,
    /// The file has no form to fill (or only an XFA form).
    NoForm,
    /// The table has no row for this file.
    NoRow,
    /// Field names in the values that the form doesn't have.
    UnknownFields(Vec<String>),
    /// Values a field can't take (`field: value`).
    BadValues(Vec<String>),
    /// The pages asked for aren't in this file.
    Pages(String),
    /// Nothing on the pages to flatten.
    NothingToFlatten,
    /// The result file exists (and wasn't to be replaced).
    Exists,
    /// Said by a plugin step.
    Plugin(String),
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::Password => f.write_str("password-protected (give its password)"),
            Reason::Signed => f.write_str(
                "skipped: the file is digitally signed and this would invalidate the signature",
            ),
            Reason::NoChangesAllowed => {
                f.write_str("skipped: the file is certified and allows no changes")
            }
            Reason::FormFillOnly => f.write_str(
                "skipped: the file is certified and allows only filling in forms and signing",
            ),
            Reason::NoForm => f.write_str("skipped: the file has no form fields to fill"),
            Reason::NoRow => f.write_str("skipped: no row for this file in the table"),
            Reason::UnknownFields(v) => write!(f, "fields not in this file: {}", v.join(", ")),
            Reason::BadValues(v) => write!(f, "values not allowed: {}", v.join(", ")),
            Reason::Pages(e) => write!(f, "skipped: {e}"),
            Reason::NothingToFlatten => f.write_str("nothing to flatten"),
            Reason::Exists => f.write_str("skipped: the result file already exists"),
            Reason::Plugin(s) => f.write_str(s),
        }
    }
}

/// A note on step `step` (0-based; `None` for the whole file).
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub step: Option<usize>,
    pub reason: Reason,
}

/// What happened to one file.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Changed: the new file's bytes; `incremental` when saved as an
    /// incremental update of a signed file (or one with usage rights),
    /// keeping its signatures.
    Done {
        bytes: Vec<u8>,
        incremental: bool,
    },
    /// No step applied: nothing written.
    Skipped,
    Failed(String),
}

/// One file's result: the outcome and the notes on its steps.
#[derive(Clone, Debug, PartialEq)]
pub struct FileRun {
    pub outcome: Outcome,
    pub notes: Vec<Note>,
}

/// Run the batch on one PDF (`name` matches it to a table row). Encrypted
/// files open with `password`, else with an empty one.
pub fn run_file(p: &Prepared, name: &str, bytes: &[u8], password: Option<&str>) -> FileRun {
    let mut notes = Vec::new();
    let fail = |e: Error, notes: Vec<Note>| FileRun {
        outcome: Outcome::Failed(e.to_string()),
        notes,
    };
    // Open it: the password given, or none, or an empty one.
    let (pdf, used) = match Pdf::from_bytes(bytes, password) {
        Ok(pdf) => {
            let used = pdf
                .was_encrypted()
                .then(|| password.unwrap_or("").to_owned());
            (pdf, used)
        }
        Err(Error::Encrypted) => match Pdf::from_bytes(bytes, Some("")) {
            Ok(pdf) => (pdf, Some(String::new())),
            Err(Error::WrongPassword | Error::Encrypted) => {
                return FileRun {
                    outcome: Outcome::Skipped,
                    notes: vec![Note {
                        step: None,
                        reason: Reason::Password,
                    }],
                };
            }
            Err(e) => return fail(e, notes),
        },
        Err(Error::WrongPassword) => {
            return FileRun {
                outcome: Outcome::Skipped,
                notes: vec![Note {
                    step: None,
                    reason: Reason::Password,
                }],
            };
        }
        Err(e) => return fail(e, notes),
    };
    let sigs = crate::sig::signatures(&pdf);
    let keep_prefix = sigs.is_some() || crate::sig::usage_rights(&pdf);
    let before = pdf.clone();
    let mut pdf = pdf;
    let mut applied = 0;
    let mut plugins = Vec::new();
    for (i, step) in p.steps.iter().enumerate() {
        let note = |notes: &mut Vec<Note>, reason| {
            notes.push(Note {
                step: Some(i),
                reason,
            })
        };
        // What a signed file allows.
        if let Some(s) = sigs {
            let refused = match step {
                Ready::Stamp { .. } if !s.allows_annotations() => Some(if s.allows_form_fill() {
                    Reason::FormFillOnly
                } else {
                    Reason::NoChangesAllowed
                }),
                Ready::Fill { .. } if !s.allows_form_fill() => Some(Reason::NoChangesAllowed),
                Ready::PageNumbers(..) | Ready::Watermark(..) | Ready::Flatten => {
                    Some(Reason::Signed)
                }
                _ => None,
            };
            if let Some(r) = refused {
                note(&mut notes, r);
                continue;
            }
        }
        let result = match step {
            Ready::Stamp {
                picture,
                natural,
                pages,
                place,
                width,
                opacity,
                signature,
            } => place_stamp(
                &mut pdf, picture, *natural, pages, place, *width, *opacity, *signature,
            ),
            Ready::Fill { table, values } => fill(&mut pdf, name, table.as_deref(), values),
            Ready::PageNumbers(o, pages) => resolve(&pdf, pages).and_then(|r| {
                crate::add_page_numbers(&mut pdf, o, &r)
                    .map(|_| Vec::new())
                    .map_err(StepError::Error)
            }),
            Ready::Watermark(o, pages) => resolve(&pdf, pages).and_then(|r| {
                crate::add_watermark(&mut pdf, o, &r)
                    .map(|_| Vec::new())
                    .map_err(StepError::Error)
            }),
            Ready::Flatten => match annot::flatten(&mut pdf, annot::FlattenWhat::Annotations) {
                Ok(0) => Err(StepError::Skip(Reason::NothingToFlatten)),
                Ok(_) => Ok(Vec::new()),
                Err(e) => Err(StepError::Error(e)),
            },
            Ready::Plugin(pl, opts) => {
                plugins.push((i, pl.clone(), opts.clone()));
                continue;
            }
        };
        match result {
            Ok(extra) => {
                applied += 1;
                extra.into_iter().for_each(|r| note(&mut notes, r));
            }
            Err(StepError::Skip(r)) => note(&mut notes, r),
            Err(StepError::Error(e)) => {
                return fail(
                    Error::Invalid(format!("step {} ({}): {e}", i + 1, p.batch.steps[i].kind())),
                    notes,
                );
            }
        }
    }
    let mut out = None;
    let mut incremental = false;
    if applied > 0 {
        let bytes = if keep_prefix {
            incremental = true;
            crate::sig::incremental_diff(bytes, &before, &pdf)
        } else {
            pdf.to_bytes_encrypted(true)
        };
        match bytes {
            Ok(b) => out = Some(b),
            Err(e) => return fail(e, notes),
        }
    }
    for (i, pl, opts) in plugins {
        let ctx = FileContext {
            name,
            password: used.as_deref(),
            signed: sigs.is_some(),
        };
        let current = out.as_deref().unwrap_or(bytes);
        match pl.apply(&opts, &ctx, current) {
            Ok(Applied::Changed(b)) => {
                out = Some(b);
                // A plugin (the sign step) appends to whatever it gets:
                // "incremental" still means the original's signatures were
                // kept, so an unsigned file stays a plain save.
                incremental = keep_prefix;
            }
            Ok(Applied::Skipped(why)) => notes.push(Note {
                step: Some(i),
                reason: Reason::Plugin(why),
            }),
            Err(e) => {
                return fail(
                    Error::Invalid(format!("step {} ({}): {e}", i + 1, pl.kind())),
                    notes,
                );
            }
        }
    }
    FileRun {
        outcome: match out {
            Some(bytes) => Outcome::Done { bytes, incremental },
            None => Outcome::Skipped,
        },
        notes,
    }
}

enum StepError {
    /// The step doesn't apply to this file.
    Skip(Reason),
    Error(Error),
}

fn resolve(pdf: &Pdf, pages: &str) -> std::result::Result<PageRanges, StepError> {
    let r = page_spec(pages).map_err(|e| StepError::Skip(Reason::Pages(e.to_string())))?;
    r.resolve(pdf.page_count())
        .map_err(|e| StepError::Skip(Reason::Pages(e.to_string())))?;
    Ok(r)
}

/// One page from `first`, `last` or a page number (from 1): its 0-based
/// index in a document of `count` pages.
pub fn one_page(spec: &str, count: u32) -> std::result::Result<u32, String> {
    let n = match spec.trim().to_ascii_lowercase().as_str() {
        "" | "first" => 1,
        "last" => count,
        other => other
            .parse::<u32>()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| format!("\"{spec}\" is not first, last or a page number"))?,
    };
    if n == 0 || n > count {
        return Err(format!(
            "page {n} is not in the file ({count} page{})",
            if count == 1 { "" } else { "s" }
        ));
    }
    Ok(n - 1)
}

/// A box placed on page `index` (0-based) as the stamp step places
/// pictures: its rectangle in user space (`[x0 y0 x1 y1]`) and the page's
/// `/Rotate`, for a `w` × `h` points box upright on the page as shown.
pub fn place_box(pdf: &Pdf, index: u32, place: &Place, w: f32, h: f32) -> Option<([f32; 4], i64)> {
    let pid = *page_ids(&pdf.doc).get(index as usize)?;
    let (pw, ph, m) = display(pdf, pid);
    let (x, y) = place_on(place, pw, ph, w, h);
    // Display points from the bottom left, then user space.
    let (dx0, dy0, dx1, dy1) = (x, ph - y - h, x + w, ph - y);
    let map = |px: f32, py: f32| (m[0] * px + m[2] * py + m[4], m[1] * px + m[3] * py + m[5]);
    let (a, b) = (map(dx0, dy0), map(dx1, dy1));
    Some((
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)],
        rotation(&pdf.doc, pid),
    ))
}

/// The page's display transform: (display width, height, matrix taking
/// display points (origin bottom-left, y up) to user space).
fn display(pdf: &Pdf, pid: lopdf::ObjectId) -> (f32, f32, [f32; 6]) {
    let [x0, y0, x1, y1] = visible_box(&pdf.doc, pid);
    let (w, h) = (x1 - x0, y1 - y0);
    match rotation(&pdf.doc, pid) {
        90 => (h, w, [0.0, 1.0, -1.0, 0.0, x0 + w, y0]),
        180 => (w, h, [-1.0, 0.0, 0.0, -1.0, x0 + w, y0 + h]),
        270 => (h, w, [0.0, -1.0, 1.0, 0.0, x0, y0 + h]),
        _ => (w, h, [1.0, 0.0, 0.0, 1.0, x0, y0]),
    }
}

/// The picture's top-left corner on a page `pw` × `ph` (display points
/// from the top-left) for a picture `w` × `h`.
pub fn place_on(place: &Place, pw: f32, ph: f32, w: f32, h: f32) -> (f32, f32) {
    match *place {
        Place::Anchor { anchor, dx, dy } => {
            let (col, row) = anchor.grid();
            let x = match col {
                0 => dx,
                1 => (pw - w) / 2.0 + dx,
                _ => pw - w - dx,
            };
            let y = match row {
                0 => dy,
                1 => (ph - h) / 2.0 + dy,
                _ => ph - h - dy,
            };
            (x, y)
        }
        Place::Point { x, y } => (x, y),
        Place::Example { example: e } => {
            // Keep the distance to the nearer edge (the middle stays the middle).
            let axis = |pos: f32, size: f32, page: f32, new_page: f32, new_size: f32| {
                let before = pos;
                let after = page - pos - size;
                if (before - after).abs() < page * 0.05 {
                    (new_page - new_size) / 2.0
                } else if before <= after {
                    before
                } else {
                    new_page - new_size - after
                }
            };
            (
                axis(e.x, e.width, e.page_width, pw, w),
                axis(e.y, e.height, e.page_height, ph, h),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn place_stamp(
    pdf: &mut Pdf,
    picture: &Picture,
    natural: (f32, f32),
    pages: &str,
    place: &Place,
    width: f32,
    opacity: f32,
    signature: bool,
) -> std::result::Result<Vec<Reason>, StepError> {
    let ranges = resolve(pdf, pages)?;
    let list = ranges
        .resolve_set(pdf.page_count())
        .map_err(|e| StepError::Skip(Reason::Pages(e.to_string())))?;
    let ids = page_ids(&pdf.doc);
    let width = match place {
        Place::Example { example } if width <= 0.0 => example.width,
        _ => width,
    };
    for p in list {
        let Some(&pid) = ids.get(p as usize - 1) else {
            continue;
        };
        let (pw, ph, m) = display(pdf, pid);
        let (mut w, mut h) = natural;
        if width > 0.0 {
            h = h * width / w.max(1e-3);
            w = width;
        } else {
            let k = (pw * 0.5 / w).min(ph * 0.5 / h).min(1.0);
            w *= k;
            h *= k;
        }
        let (left, top) = place_on(place, pw, ph, w, h);
        // Display (y up) → user space.
        let to_user = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let bottom = ph - top - h;
        let (cx, cy) = to_user(left + w / 2.0, bottom + h / 2.0);
        let rect = [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0];
        let turn = rotation(&pdf.doc, pid) as f32;
        let mut a = match picture {
            Picture::Image(bytes) => Annot::new(
                Kind::Stamp {
                    art: Art::Image(bytes.clone()),
                    rotate: turn,
                    signature,
                },
                rect,
                [0.0, 0.0, 0.0],
            ),
            Picture::Ink(strokes) => {
                let strokes = strokes
                    .iter()
                    .map(|s| {
                        s.iter()
                            .map(|q| {
                                let (x, y) = to_user(left + q[0] * w, bottom + q[1] * h);
                                [x, y]
                            })
                            .collect()
                    })
                    .collect();
                let (ax, ay) = to_user(left, bottom);
                let (bx, by) = to_user(left + w, bottom + h);
                let mut a = Annot::new(Kind::Ink(strokes), [ax, ay, bx, by], [0.05, 0.1, 0.35]);
                a.width = 1.5;
                a
            }
        };
        a.opacity = opacity;
        if signature {
            a.subject = "Signature".into();
        }
        let page = p - 1;
        let mut marks = annot::read_page(pdf, page);
        marks.push(a);
        annot::apply_page(pdf, page, &marks).map_err(StepError::Error)?;
    }
    Ok(Vec::new())
}

/// Truthy words for check boxes (English and Indonesian).
fn truthy(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "yes" | "y" | "true" | "1" | "x" | "on" | "checked" | "ya" | "v" | "✓" | "✔" => {
            Some(true)
        }
        "no" | "n" | "false" | "0" | "off" | "" | "unchecked" | "tidak" => Some(false),
        _ => None,
    }
}

fn fill(
    pdf: &mut Pdf,
    name: &str,
    table: Option<&Table>,
    fixed: &Values,
) -> std::result::Result<Vec<Reason>, StepError> {
    let mut values = fixed.clone();
    if let Some(t) = table {
        match t.values_for(name) {
            Some(row) => values.extend(row.iter().map(|(k, v)| (k.clone(), v.clone()))),
            None if fixed.is_empty() => return Err(StepError::Skip(Reason::NoRow)),
            None => {}
        }
    }
    let form = match crate::form::read_form(pdf) {
        Some(f) if !f.xfa_only() && !f.fields.is_empty() => f,
        _ => return Err(StepError::Skip(Reason::NoForm)),
    };
    let mut set: HashMap<String, FieldValue> = HashMap::new();
    let mut unknown = Vec::new();
    let mut bad = Vec::new();
    for (key, text) in &values {
        // Exact name first, then without case.
        let field = form.field(key).or_else(|| {
            form.fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(key))
        });
        let Some(field) = field else {
            unknown.push(key.clone());
            continue;
        };
        let option = |t: &str| -> Option<String> {
            field
                .options
                .iter()
                .find(|(e, s)| e.eq_ignore_ascii_case(t.trim()) || s.eq_ignore_ascii_case(t.trim()))
                .map(|(e, _)| e.clone())
        };
        let on_states: Vec<String> = field
            .widgets
            .iter()
            .filter_map(|w| w.on_state.clone())
            .collect();
        let v = match &field.kind {
            FieldKind::Text { .. } | FieldKind::Combo { edit: true } => Some(match option(text) {
                Some(e) if matches!(field.kind, FieldKind::Combo { .. }) => {
                    FieldValue::Choice(vec![e])
                }
                _ if matches!(field.kind, FieldKind::Combo { .. }) => {
                    FieldValue::Choice(vec![text.clone()])
                }
                _ => FieldValue::Text(text.clone()),
            }),
            FieldKind::Combo { edit: false } => option(text).map(|e| FieldValue::Choice(vec![e])),
            FieldKind::List { multi } => {
                let parts: Vec<&str> = if *multi {
                    text.split([';', '\n'])
                        .filter(|s| !s.trim().is_empty())
                        .collect()
                } else {
                    vec![text.as_str()]
                };
                let chosen: Option<Vec<String>> = parts.iter().map(|p| option(p)).collect();
                chosen.map(FieldValue::Choice)
            }
            FieldKind::Check => {
                let on = on_states.first().cloned().unwrap_or_else(|| "Yes".into());
                if on.eq_ignore_ascii_case(text.trim()) {
                    Some(FieldValue::State(on))
                } else {
                    truthy(text).map(|t| FieldValue::State(if t { on } else { "Off".into() }))
                }
            }
            FieldKind::Radio => {
                let t = text.trim();
                if let Some(s) = on_states.iter().find(|s| s.eq_ignore_ascii_case(t)) {
                    Some(FieldValue::State(s.clone()))
                } else if let Some(e) = option(t)
                    && let Some(i) = field.options.iter().position(|(x, _)| *x == e)
                    && let Some(s) = on_states.get(i)
                {
                    // Radio buttons with /Opt: the value is an option's place.
                    Some(FieldValue::State(s.clone()))
                } else if truthy(t) == Some(false) {
                    Some(FieldValue::State("Off".into()))
                } else {
                    None
                }
            }
            FieldKind::Push | FieldKind::Signature => {
                unknown.push(key.clone());
                continue;
            }
        };
        match v {
            Some(v) => {
                set.insert(field.name.clone(), v);
            }
            None => bad.push(format!("{key}: {text}")),
        }
    }
    let mut notes = Vec::new();
    if !unknown.is_empty() {
        notes.push(Reason::UnknownFields(unknown));
    }
    if !bad.is_empty() {
        notes.push(Reason::BadValues(bad));
    }
    if set.is_empty() {
        return Err(StepError::Skip(
            notes.into_iter().next().unwrap_or(Reason::NoForm),
        ));
    }
    crate::form::set_values(pdf, &set).map_err(StepError::Error)?;
    Ok(notes)
}

// ── many files ─────────────────────────────────────────────────────────────

/// What to do when a result file exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    Replace,
    Skip,
}

/// One file's result in a batch run.
#[derive(Clone, Debug, PartialEq)]
pub struct FileResult {
    pub input: PathBuf,
    /// Where the result was written (done files).
    pub output: Option<PathBuf>,
    pub status: Status,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Written; `incremental` for signed files kept valid.
    Done {
        incremental: bool,
    },
    Skipped,
    Failed(String),
}

impl FileResult {
    /// A one-line description in English (for the command line).
    pub fn describe(&self, steps: &[Step]) -> String {
        let name = self.input.display();
        let mut s = match &self.status {
            Status::Done { incremental } => format!(
                "{name}: done → {}{}",
                self.output
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                if *incremental {
                    " (signed: saved as an incremental update, signatures kept)"
                } else {
                    ""
                }
            ),
            Status::Skipped => format!("{name}: skipped"),
            Status::Failed(e) => format!("{name}: failed: {e}"),
        };
        for n in &self.notes {
            match n.step {
                Some(i) => s.push_str(&format!(
                    "\n  step {} ({}): {}",
                    i + 1,
                    steps.get(i).map_or("", Step::kind),
                    n.reason
                )),
                None => s.push_str(&format!("\n  {}", n.reason)),
            }
        }
        s
    }
}

/// The result files that exist already (to ask before replacing them).
pub fn existing_outputs(output: &Output, inputs: &[PathBuf]) -> Vec<PathBuf> {
    output
        .paths_for(inputs)
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

/// Run the batch on `inputs`, writing each result where `output` says;
/// `progress` is called after each file. Stops early (leaving the rest
/// out of the results) when `cancel` is set.
pub fn run(
    p: &Prepared,
    inputs: &[PathBuf],
    output: &Output,
    existing: Existing,
    password: Option<&str>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, &FileResult),
) -> Vec<FileResult> {
    let outs = output.paths_for(inputs);
    if let Some(dir) = &output.folder {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut results = Vec::with_capacity(inputs.len());
    for (i, (input, out)) in inputs.iter().zip(outs).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let r = run_one(p, input, &out, existing, password);
        progress(i, &r);
        results.push(r);
    }
    results
}

fn run_one(
    p: &Prepared,
    input: &Path,
    out: &Path,
    existing: Existing,
    password: Option<&str>,
) -> FileResult {
    let mut r = FileResult {
        input: input.to_path_buf(),
        output: None,
        status: Status::Skipped,
        notes: Vec::new(),
    };
    if existing == Existing::Skip && out.exists() {
        r.notes.push(Note {
            step: None,
            reason: Reason::Exists,
        });
        return r;
    }
    let bytes = match crate::read(input) {
        Ok(b) => b,
        Err(e) => {
            r.status = Status::Failed(e.to_string());
            return r;
        }
    };
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let run = run_file(p, &name, &bytes, password);
    r.notes = run.notes;
    r.status = match run.outcome {
        Outcome::Done { bytes, incremental } => match crate::write_atomic(out, &bytes) {
            Ok(()) => {
                r.output = Some(out.to_path_buf());
                Status::Done { incremental }
            }
            Err(e) => Status::Failed(e.to_string()),
        },
        Outcome::Skipped => Status::Skipped,
        Outcome::Failed(e) => Status::Failed(e),
    };
    r
}

/// The PDFs in `dir` (not in subfolders), sorted by name.
pub fn pdfs_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
