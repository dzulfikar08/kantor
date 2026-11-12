//! A presentation: slides, read-only layouts and masters, the original
//! package, and `.pptx` open/save.
//!
//! # Round-trip safety (plan §5)
//!
//! Saving starts from the original zip and **raw-copies every part it
//! doesn't rewrite** (masters, layouts, themes, notes, media, charts,
//! custom XML...). It rewrites only:
//!
//! - slides whose shapes changed: the shape tree's other content and
//!   everything around it (background, transitions, timing, extensions)
//!   is copied from the source text, unchanged shapes are copied verbatim,
//!   moved shapes get only their `a:xfrm` replaced, edited text bodies
//!   only their paragraphs (with each paragraph's and run's unmodelled
//!   attributes and children written back) and recoloured shapes only the
//!   fill and the outline's colour and width in `p:spPr`;
//! - notes slides whose notes text changed (only the notes body's
//!   paragraphs); new notes parts get their relationships and content
//!   type, and a notes master (with a copy of the theme) is added to
//!   `presentation.xml` when the file has none;
//! - `presentation.xml` (only `p:sldIdLst`), its relationships and
//!   `[Content_Types].xml` when slides are added, removed or reordered;
//! - relationship parts of slides that gained a picture.
//!
//! What Kantor can't keep through a save is reported by
//! [`Presentation::unsupported_content`], and Save then goes to Save As:
//! the original file is never overwritten.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use docs_engine::Rgb;
use roxmltree::Node;

use crate::fill::Fill;
use crate::history::History;
use crate::model::*;
use crate::shape::{ParseCtx, parse_shape, parse_tree, write_shape};
use crate::template;
use crate::theme::{ClrMap, ColorCtx, ListStyle, Theme};
use crate::xml::*;

/// A slide master (read-only).
#[derive(Debug)]
pub struct Master {
    pub part: String,
    pub shapes: Vec<Shape>,
    pub bg: Option<Rgb>,
    /// The background fill as read (`p:bg`).
    pub background: Option<Fill>,
    pub(crate) theme: Arc<Theme>,
    pub(crate) theme_part: Option<String>,
    pub(crate) clr_map: ClrMap,
    pub(crate) title_style: ListStyle,
    pub(crate) body_style: ListStyle,
    pub(crate) other_style: ListStyle,
    /// Table styles with this master's theme colours.
    pub table_styles: Arc<crate::frame::TableStyles>,
}

/// A slide layout (read-only).
#[derive(Debug)]
pub struct Layout {
    pub name: String,
    /// `type` attribute (`title`, `obj`, `twoObj`, ...).
    pub kind: Option<String>,
    pub part: String,
    pub master: usize,
    pub shapes: Vec<Shape>,
    pub bg: Option<Rgb>,
    /// The background fill as read (`p:bg`).
    pub background: Option<Fill>,
    pub show_master_sp: bool,
}

/// How a slide's XML is put back together.
#[derive(Debug)]
pub(crate) struct SlideSrc {
    /// Everything before the first shape of the shape tree.
    pub prefix: String,
    /// Everything after the last shape.
    pub suffix: String,
    /// The shapes as read (unchanged slides are copied raw).
    pub shapes: Vec<Shape>,
    pub rels: Vec<Rel>,
}

/// Where a slide was in the original file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    /// Relationship id in `presentation.xml.rels`.
    pub rid: String,
    /// The original `p:sldId` element.
    pub sld_id_raw: String,
}

/// A slide's speaker notes (`p:notes`): the notes body placeholder is
/// modelled (edited like any text body); everything around it is kept.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Notes {
    pub part: String,
    /// The part is in the original file (else it's new and gets its
    /// relationships and content type on save).
    pub existing: bool,
    pub prefix: Arc<str>,
    pub suffix: Arc<str>,
    /// The notes body placeholder; `None` when the notes have none (kept
    /// as they are, not editable).
    pub body: Option<Shape>,
}

impl Notes {
    fn unchanged(&self) -> bool {
        self.body.as_ref().is_none_or(Shape::unchanged)
    }

    fn xml(&self) -> String {
        let mut s = String::from(&*self.prefix);
        if let Some(b) = &self.body {
            write_shape(&mut s, b);
        }
        s.push_str(&self.suffix);
        s
    }
}

/// Read a notes slide; `None` if its XML can't be read.
pub(crate) fn read_notes(
    xml: &str,
    part: &str,
    rels: &[Rel],
    master: &Master,
    existing: bool,
) -> Option<Notes> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let tree = path(doc.root_element(), &[(NS_P, "cSld"), (NS_P, "spTree")])?;
    let cx = ParseCtx {
        src: xml,
        part,
        rels,
        colors: ColorCtx {
            theme: &master.theme,
            map: &master.clr_map,
            ph: None,
        },
    };
    let body = tree
        .children()
        .filter(|n| is(*n, NS_P, "sp"))
        .find_map(|n| {
            let s = parse_shape(n, &cx);
            let ok = s.ph.as_ref().is_some_and(|p| p.ty() == "body") && s.text.is_some();
            ok.then_some((n.range(), s))
        });
    Some(match body {
        Some((range, shape)) => Notes {
            part: part.to_owned(),
            existing,
            prefix: Arc::from(&xml[..range.start]),
            suffix: Arc::from(&xml[range.end..]),
            body: Some(shape),
        },
        None => Notes {
            part: part.to_owned(),
            existing,
            prefix: Arc::from(xml),
            suffix: Arc::from(""),
            body: None,
        },
    })
}

#[derive(Clone, Debug)]
pub struct Slide {
    /// `p:sldId/@id`.
    pub sld_id: u32,
    /// Part name, e.g. `ppt/slides/slide3.xml`.
    pub part: String,
    pub layout: usize,
    pub shapes: Vec<Shape>,
    pub bg: Option<Rgb>,
    /// The background fill as read (`p:bg`).
    pub background: Option<Fill>,
    pub show_master_sp: bool,
    /// Hidden in the slide show (`show="0"`); read only, kept in the XML.
    pub hidden: bool,
    /// Why the slide can't be edited (its XML is kept as is), if so.
    pub locked: Option<String>,
    pub(crate) rels: Arc<Vec<Rel>>,
    pub(crate) src: Arc<SlideSrc>,
    pub(crate) origin: Option<Origin>,
    pub(crate) notes: Option<Notes>,
}

impl PartialEq for Slide {
    fn eq(&self, o: &Self) -> bool {
        self.sld_id == o.sld_id
            && self.part == o.part
            && self.layout == o.layout
            && self.shapes == o.shapes
            && self.rels == o.rels
            && Arc::ptr_eq(&self.src, &o.src)
            && self.origin == o.origin
            && self.notes == o.notes
    }
}

impl Slide {
    /// Whether the slide's part can be copied from the original file.
    fn unchanged(&self) -> bool {
        self.origin.is_some() && self.shapes == self.src.shapes && *self.rels == self.src.rels
    }

    /// The slide's XML as it would be saved.
    pub(crate) fn xml(&self) -> String {
        let mut s = String::with_capacity(self.src.prefix.len() + self.src.suffix.len() + 4096);
        s.push_str(&self.src.prefix);
        if self.locked.is_none() {
            for shape in &self.shapes {
                write_shape(&mut s, shape);
            }
        }
        s.push_str(&self.src.suffix);
        s
    }

    /// Declare namespace prefixes on the slide's root element.
    pub(crate) fn declare_ns(&mut self, ns: &[(String, String)]) {
        let mut prefix = self.src.prefix.clone();
        let Some(start) = prefix.find("<p:sld") else {
            return;
        };
        let Some(len) = tag_end(&prefix[start..]) else {
            return;
        };
        let mut decl = String::new();
        write_attrs(&mut decl, ns);
        let at = start + len
            - if prefix[start..start + len].ends_with("/>") {
                2
            } else {
                1
            };
        prefix.insert_str(at, &decl);
        self.src = Arc::new(SlideSrc {
            prefix,
            suffix: self.src.suffix.clone(),
            shapes: self.src.shapes.clone(),
            rels: self.src.rels.clone(),
        });
    }

    pub(crate) fn next_shape_id(&self) -> u32 {
        self.shapes.iter().map(Shape::max_id).max().unwrap_or(1) + 1
    }
}

/// The original package and what was read from it.
#[derive(Debug)]
pub(crate) struct Package {
    pub bytes: Arc<Vec<u8>>,
    pub names: HashSet<String>,
    pub ct_raw: String,
    pub pres_part: String,
    pub pres_xml: String,
    pub pres_rels: Vec<Rel>,
    /// The notes master part, if the file has one.
    pub notes_master: Option<String>,
    /// Slide parts in order, with their relationships, as in the file.
    pub orig_slides: Vec<(String, Vec<Rel>)>,
    pub has_sections: bool,
    pub has_custom_shows: bool,
}

pub struct Presentation {
    /// Identifies this presentation (copied shapes know where they came from).
    pub(crate) deck: u64,
    pub size: SlideSize,
    pub(crate) slides: Vec<Arc<Slide>>,
    pub masters: Vec<Master>,
    pub layouts: Vec<Layout>,
    pub(crate) default_style: ListStyle,
    pub(crate) pkg: Package,
    /// Media added in this session (part name → bytes).
    pub(crate) media: HashMap<String, Arc<Vec<u8>>>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) history: History,
    pub(crate) unsupported: Vec<String>,
    pub(crate) next_slide_num: u32,
    pub(crate) next_sld_id: u32,
}

fn read_zip_text(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<Option<String>> {
    let mut file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut s = String::new();
    file.read_to_string(&mut s)
        .with_context(|| format!("reading {name}"))?;
    Ok(Some(s))
}

/// Background colour of a `p:cSld`.
fn background(csld: Node, colors: &ColorCtx) -> Option<Rgb> {
    let bg = child(csld, NS_P, "bg")?;
    if let Some(pr) = child(bg, NS_P, "bgPr") {
        return pr
            .children()
            .filter(|c| c.is_element())
            .find_map(|c| colors.fill(c));
    }
    child(bg, NS_P, "bgRef").and_then(|r| colors.first_color(r))
}

/// The background fill of a `p:cSld` (`p:bgPr` or a theme `p:bgRef`);
/// pictures resolve against `rels` of `part`.
fn background_fill(csld: Node, colors: &ColorCtx, part: &str, rels: &[Rel]) -> Option<Fill> {
    let bg = child(csld, NS_P, "bg")?;
    if let Some(pr) = child(bg, NS_P, "bgPr") {
        let resolve = |id: &str| rels.iter().find(|r| r.id == id).and_then(|r| r.part(part));
        return pr
            .children()
            .filter(|c| c.is_element())
            .find_map(|c| colors.full_fill(c, &resolve));
    }
    let r = child(bg, NS_P, "bgRef")?;
    let ph = colors.first_color(r);
    colors
        .style_fill(attr_i64(r, "idx").unwrap_or(0), ph)
        .or_else(|| ph.map(|c| Fill::Solid(c, 255)))
}

/// Whether the root declares the usual `p:`, `a:` and `r:` prefixes (the
/// shape XML Kantor writes uses them).
fn usual_prefixes(root: Node) -> bool {
    root.lookup_namespace_uri(Some("p")) == Some(NS_P)
        && root.lookup_namespace_uri(Some("a")) == Some(NS_A)
        && matches!(root.lookup_namespace_uri(Some("r")), Some(NS_R) | None)
}

/// Add `xmlns:r` to a root start tag that lacks it (needed for pictures).
fn ensure_r_decl(prefix: &mut String, root_start: usize) {
    let Some(len) = tag_end(&prefix[root_start..]) else {
        return;
    };
    let tag = &prefix[root_start..root_start + len];
    if tag.contains("xmlns:r=") {
        return;
    }
    let at = root_start + len - if tag.ends_with("/>") { 2 } else { 1 };
    prefix.insert_str(at, &format!(" xmlns:r=\"{NS_R}\""));
}

struct Reader<'z> {
    zip: zip::ZipArchive<Cursor<&'z [u8]>>,
}

impl Reader<'_> {
    fn text(&mut self, name: &str) -> anyhow::Result<Option<String>> {
        read_zip_text(&mut self.zip, name)
    }

    fn rels(&mut self, part: &str) -> anyhow::Result<Vec<Rel>> {
        Ok(self
            .text(&rels_name(part))?
            .map(|x| parse_rels(&x))
            .unwrap_or_default())
    }
}

impl Presentation {
    /// A new presentation (16:9, one title slide).
    pub fn new() -> Self {
        // The template is fixed and covered by tests.
        Self::from_bytes(template::new_package()).expect("built-in template opens")
    }

    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let mut p = Self::from_bytes(bytes).with_context(|| {
            format!(
                "{} is not a PowerPoint file Kantor can read",
                path.display()
            )
        })?;
        p.path = Some(path.to_path_buf());
        Ok(p)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let bytes = Arc::new(bytes);
        let zip =
            zip::ZipArchive::new(Cursor::new(bytes.as_slice())).context("not a zip package")?;
        let names: HashSet<String> = zip
            .file_names()
            .filter_map(|n| n.ok().map(|n| n.into_owned()))
            .collect();
        let mut rd = Reader { zip };
        let mut unsupported = Vec::new();

        let ct_raw = rd
            .text("[Content_Types].xml")?
            .context("no [Content_Types].xml")?;
        let root_rels = rd
            .text("_rels/.rels")?
            .map(|x| parse_rels(&x))
            .unwrap_or_default();
        let office = root_rels
            .iter()
            .find(|r| r.kind() == "officeDocument")
            .context("no main document relationship")?;
        if office.ty.contains("purl.oclc.org") {
            bail!("Strict Open XML presentations aren't supported yet");
        }
        let pres_part = resolve("", &office.target);
        let pres_xml = rd.text(&pres_part)?.context("no presentation part")?;
        let pres_rels = rd.rels(&pres_part)?;
        let pdoc = roxmltree::Document::parse(&pres_xml).context("presentation.xml")?;
        let proot = pdoc.root_element();
        if !is(proot, NS_P, "presentation") {
            bail!("not a PresentationML document");
        }
        let size = child(proot, NS_P, "sldSz")
            .and_then(|s| {
                Some(SlideSize {
                    cx: attr_i64(s, "cx")?,
                    cy: attr_i64(s, "cy")?,
                })
            })
            .unwrap_or(SlideSize::WIDE);
        let has_sections = proot
            .descendants()
            .any(|d| d.tag_name().name() == "sectionLst");
        let has_custom_shows = child(proot, NS_P, "custShowLst").is_some();
        let rel_part = |rid: &str| {
            pres_rels
                .iter()
                .find(|r| r.id == rid)
                .and_then(|r| r.part(&pres_part))
        };

        // Masters and their layouts.
        let mut masters = Vec::new();
        let mut layouts: Vec<Layout> = Vec::new();
        let mut layout_index: HashMap<String, usize> = HashMap::new();
        let master_parts: Vec<String> = child(proot, NS_P, "sldMasterIdLst")
            .map(|l| {
                children(l, NS_P, "sldMasterId")
                    .filter_map(|m| r_attr(m, "id").and_then(rel_part))
                    .collect()
            })
            .unwrap_or_default();
        let table_styles = match pres_rels
            .iter()
            .find(|r| r.kind() == "tableStyles")
            .and_then(|r| r.part(&pres_part))
        {
            Some(p) => rd.text(&p)?,
            None => None,
        };
        for mpart in master_parts {
            let mi = masters.len();
            let (master, layout_parts) = read_master(&mut rd, &mpart, table_styles.as_deref())?;
            masters.push(master);
            for lpart in layout_parts {
                if layout_index.contains_key(&lpart) {
                    continue;
                }
                if let Some(l) = read_layout(&mut rd, &lpart, mi, &masters[mi])? {
                    layout_index.insert(lpart, layouts.len());
                    layouts.push(l);
                }
            }
        }
        if masters.is_empty() {
            bail!("the presentation has no slide master");
        }

        let default_style = {
            let m = &masters[0];
            let colors = ColorCtx {
                theme: &m.theme,
                map: &m.clr_map,
                ph: None,
            };
            child(proot, NS_P, "defaultTextStyle")
                .map(|d| ListStyle::parse(d, &colors))
                .unwrap_or_default()
        };

        // Slides.
        let mut slides = Vec::new();
        let mut orig_slides = Vec::new();
        let mut max_sld_id = 255u32;
        if let Some(list) = child(proot, NS_P, "sldIdLst") {
            for s in children(list, NS_P, "sldId") {
                let Some(rid) = r_attr(s, "id") else { continue };
                let Some(part) = rel_part(rid) else { continue };
                let sld_id = attr_i64(s, "id").unwrap_or(0).max(0) as u32;
                max_sld_id = max_sld_id.max(sld_id);
                let Some(xml) = rd.text(&part)? else {
                    unsupported.push(format!("a missing slide part ({part})"));
                    continue;
                };
                let rels = rd.rels(&part)?;
                let lpart = rels
                    .iter()
                    .find(|r| r.kind() == "slideLayout")
                    .and_then(|r| r.part(&part));
                let layout = match lpart {
                    Some(lp) => match layout_index.get(&lp) {
                        Some(&i) => i,
                        None => {
                            // A layout no master lists: read it with its own master.
                            let mi = rd
                                .rels(&lp)?
                                .iter()
                                .find(|r| r.kind() == "slideMaster")
                                .and_then(|r| r.part(&lp))
                                .and_then(|mp| masters.iter().position(|m| m.part == mp))
                                .unwrap_or(0);
                            match read_layout(&mut rd, &lp, mi, &masters[mi])? {
                                Some(l) => {
                                    layout_index.insert(lp, layouts.len());
                                    layouts.push(l);
                                    layouts.len() - 1
                                }
                                None => 0,
                            }
                        }
                    },
                    None => 0,
                };
                if layouts.is_empty() {
                    bail!("the presentation has no slide layouts");
                }
                let master = &masters[layouts[layout].master];
                let origin = Origin {
                    rid: rid.to_owned(),
                    sld_id_raw: raw(&pres_xml, s).to_owned(),
                };
                orig_slides.push((part.clone(), rels.clone()));
                let notes_part = rels
                    .iter()
                    .find(|r| r.kind() == "notesSlide")
                    .and_then(|r| r.part(&part));
                let mut notes = None;
                if let Some(np) = notes_part
                    && let Some(nx) = rd.text(&np)?
                {
                    let nrels = rd.rels(&np)?;
                    notes = read_notes(&nx, &np, &nrels, master, true);
                    if notes.is_none() {
                        unsupported.push(format!("speaker notes that can't be read ({np})"));
                    }
                }
                let mut slide = read_slide(&xml, &part, rels, layout, master, sld_id, Some(origin));
                slide.notes = notes;
                slides.push(Arc::new(slide));
            }
        }
        if layouts.is_empty() {
            bail!("the presentation has no slide layouts");
        }
        let notes_master = pres_rels
            .iter()
            .find(|r| r.kind() == "notesMaster")
            .and_then(|r| r.part(&pres_part));
        let next_slide_num = names
            .iter()
            .filter_map(|n| {
                n.strip_prefix("ppt/slides/slide")?
                    .strip_suffix(".xml")?
                    .parse::<u32>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        Ok(Self {
            deck: crate::clip::new_deck_id(),
            size,
            slides,
            masters,
            layouts,
            default_style,
            pkg: Package {
                bytes,
                names,
                ct_raw,
                pres_part,
                pres_xml,
                pres_rels,
                notes_master,
                orig_slides,
                has_sections,
                has_custom_shows,
            },
            media: HashMap::new(),
            path: None,
            history: History::default(),
            unsupported,
            next_slide_num,
            next_sld_id: max_sld_id + 1,
        })
    }

    // ── facts ──────────────────────────────────────────────────────────────

    pub fn slides(&self) -> &[Arc<Slide>] {
        &self.slides
    }

    pub fn slide(&self, i: usize) -> &Slide {
        &self.slides[i]
    }

    pub fn len(&self) -> usize {
        self.slides.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slides.is_empty()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.pptx".to_owned())
    }

    pub fn suggested_file_name(&self) -> String {
        match self.path.as_deref().and_then(|p| p.file_stem()) {
            Some(stem) if self.has_unsupported_content() => {
                format!("{} (Kantor).pptx", stem.to_string_lossy())
            }
            Some(stem) => format!("{}.pptx", stem.to_string_lossy()),
            None => "Untitled.pptx".to_owned(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// Changes with every edit, undo and redo, so callers can tell the
    /// presentation changed.
    pub fn revision(&self) -> u64 {
        self.history.changes()
    }

    /// A recovery copy (see [`Self::to_bytes`]) as the presentation it was
    /// made from: `original` with unsaved changes. `unsupported` is what the
    /// original had that Kantor can't keep, so Save still won't overwrite it.
    pub fn open_recovered(
        bytes: Vec<u8>,
        original: Option<PathBuf>,
        unsupported: Vec<String>,
    ) -> anyhow::Result<Self> {
        let mut p = Self::from_bytes(bytes).context("reading the recovery copy")?;
        p.path = original;
        for u in unsupported {
            if !p.unsupported_content().contains(&u) {
                p.unsupported.push(u);
            }
        }
        p.history.mark_modified();
        Ok(p)
    }

    fn structure_changed(&self) -> bool {
        self.slides.len() != self.pkg.orig_slides.len()
            || self
                .slides
                .iter()
                .zip(&self.pkg.orig_slides)
                .any(|(s, (p, _))| s.origin.is_none() || s.part != *p)
    }

    /// Content Kantor can't keep through a save (short descriptions).
    pub fn unsupported_content(&self) -> Vec<String> {
        let mut v = self.unsupported.clone();
        if self.structure_changed() {
            if self.pkg.has_sections {
                v.push(
                    "slide sections (dropped when slides are added, removed or reordered)".into(),
                );
            }
            if self.pkg.has_custom_shows {
                v.push(
                    "custom slide shows (dropped when slides are added, removed or reordered)"
                        .into(),
                );
            }
        }
        v
    }

    pub fn has_unsupported_content(&self) -> bool {
        !self.unsupported_content().is_empty()
    }

    /// Save has to ask for a new file (no file yet, or unsupported content).
    pub fn needs_save_as(&self) -> bool {
        self.path.is_none() || self.has_unsupported_content()
    }

    /// Bytes of a package part (media), including pictures added this session.
    pub fn part_bytes(&self, part: &str) -> Option<Arc<Vec<u8>>> {
        if let Some(b) = self.media.get(part) {
            return Some(b.clone());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(self.pkg.bytes.as_slice())).ok()?;
        let mut f = zip.by_name(part).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(Arc::new(v))
    }

    // ── saving ─────────────────────────────────────────────────────────────

    pub fn save(&mut self) -> anyhow::Result<()> {
        let Some(path) = self.path.clone() else {
            bail!("This presentation has no file yet; use Save As");
        };
        if self.has_unsupported_content() {
            bail!(
                "{} has content Kantor can't keep ({}); use Save As to write a new file",
                self.title(),
                self.unsupported_content().join(", ")
            );
        }
        self.write_to(&path)
    }

    /// Save to `path` (refuses to overwrite the original when content would be lost).
    pub fn save_as(&mut self, path: &Path) -> anyhow::Result<()> {
        if self.has_unsupported_content()
            && let Some(orig) = self.path.as_deref()
            && same_file(orig, path)
        {
            bail!(
                "Not overwriting {}: it has content Kantor can't keep ({}). Choose a new name.",
                self.title(),
                self.unsupported_content().join(", ")
            );
        }
        self.write_to(path)?;
        self.path = Some(path.to_path_buf());
        Ok(())
    }

    fn write_to(&mut self, path: &Path) -> anyhow::Result<()> {
        let bytes = self.to_bytes()?;
        atomic_write(path, &bytes)?;
        self.history.mark_saved();
        Ok(())
    }

    /// Serialise into a `.pptx`.
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let pkg = &self.pkg;
        let mut ct = ContentTypes::parse(&pkg.ct_raw);
        let mut out_parts: Vec<(String, Vec<u8>)> = Vec::new();
        let mut drop: HashSet<String> = HashSet::new();

        // Slides.
        let live: HashSet<&str> = self.slides.iter().map(|s| s.part.as_str()).collect();
        for slide in &self.slides {
            if slide.unchanged() {
                continue;
            }
            out_parts.push((slide.part.clone(), slide.xml().into_bytes()));
            if slide.origin.is_none() || *slide.rels != slide.src.rels {
                out_parts.push((rels_name(&slide.part), write_rels(&slide.rels).into_bytes()));
            }
            ct.set_override(&slide.part, CT_SLIDE);
        }
        // Slides that were removed: drop them with their notes and comments.
        let kept_targets: HashSet<String> = self
            .slides
            .iter()
            .flat_map(|s| {
                s.rels
                    .iter()
                    .filter_map(|r| r.part(&s.part))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (part, rels) in &pkg.orig_slides {
            if live.contains(part.as_str()) {
                continue;
            }
            drop.insert(part.clone());
            drop.insert(rels_name(part));
            ct.remove_override(part);
            for r in rels {
                if matches!(r.kind(), "notesSlide" | "comments")
                    && let Some(t) = r.part(part)
                    && !kept_targets.contains(&t)
                {
                    drop.insert(rels_name(&t));
                    ct.remove_override(&t);
                    drop.insert(t);
                }
            }
        }
        // Speaker notes: edited ones are rewritten; new ones get their
        // relationships (notes master, slide) and content type, and the
        // presentation gets a notes master if it has none.
        let any_new_notes = self
            .slides
            .iter()
            .any(|s| s.notes.as_ref().is_some_and(|n| !n.existing));
        let mut new_master = None;
        let master_part = match &pkg.notes_master {
            Some(m) => Some(m.clone()),
            None if any_new_notes => {
                let free = |dir: &str, stem: &str| {
                    (1..)
                        .map(|i| format!("ppt/{dir}/{stem}{i}.xml"))
                        .find(|p| !pkg.names.contains(p))
                        .expect("a free part name")
                };
                let m = free("notesMasters", "notesMaster");
                let theme = free("theme", "theme");
                let theme_bytes = self.masters[0]
                    .theme_part
                    .as_deref()
                    .and_then(|t| self.part_bytes(t))
                    .map(|b| b.as_ref().clone())
                    .unwrap_or_else(|| template::theme().into_bytes());
                out_parts.push((m.clone(), template::notes_master().into_bytes()));
                let mrels = vec![Rel {
                    id: "rId1".into(),
                    ty: rel_type("theme"),
                    target: relative(&m, &theme),
                    external: false,
                }];
                out_parts.push((rels_name(&m), write_rels(&mrels).into_bytes()));
                out_parts.push((theme.clone(), theme_bytes));
                ct.set_override(&m, CT_NOTES_MASTER);
                ct.set_override(&theme, CT_THEME);
                new_master = Some(m.clone());
                Some(m)
            }
            None => None,
        };
        for slide in &self.slides {
            let Some(n) = &slide.notes else { continue };
            if n.existing {
                if !n.unchanged() {
                    out_parts.push((n.part.clone(), n.xml().into_bytes()));
                }
                continue;
            }
            let Some(m) = &master_part else { continue };
            out_parts.push((n.part.clone(), n.xml().into_bytes()));
            let nrels = vec![
                Rel {
                    id: "rId1".into(),
                    ty: rel_type("notesMaster"),
                    target: relative(&n.part, m),
                    external: false,
                },
                Rel {
                    id: "rId2".into(),
                    ty: rel_type("slide"),
                    target: relative(&n.part, &slide.part),
                    external: false,
                },
            ];
            out_parts.push((rels_name(&n.part), write_rels(&nrels).into_bytes()));
            ct.set_override(&n.part, CT_NOTES);
        }
        // Media added this session that some slide uses.
        for (part, bytes) in &self.media {
            if kept_targets.contains(part) {
                let ext = part.rsplit('.').next().unwrap_or("png");
                if !ct.covers(part) {
                    ct.ensure_default(ext, media_type(ext));
                }
                out_parts.push((part.clone(), bytes.as_ref().clone()));
            }
        }
        // presentation.xml and its relationships.
        if self.structure_changed() || new_master.is_some() {
            let (xml, rels) = self.presentation_parts(new_master.as_deref())?;
            out_parts.push((pkg.pres_part.clone(), xml.into_bytes()));
            out_parts.push((rels_name(&pkg.pres_part), write_rels(&rels).into_bytes()));
        }
        if ct != ContentTypes::parse(&pkg.ct_raw) {
            out_parts.push(("[Content_Types].xml".into(), ct.write().into_bytes()));
        }

        let mut src = zip::ZipArchive::new(Cursor::new(pkg.bytes.as_slice()))?;
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut pending: HashMap<String, Vec<u8>> = out_parts.into_iter().collect();
        for i in 0..src.len() {
            let file = src.by_index_raw(i)?;
            let name = file.name()?.into_owned();
            if drop.contains(&name) {
                continue;
            }
            match pending.remove(&name) {
                Some(bytes) => {
                    out.start_file(name.as_str(), opts)?;
                    out.write_all(&bytes)?;
                }
                None => out.raw_copy_file(file)?,
            }
        }
        let mut rest: Vec<_> = pending.into_iter().collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, bytes) in rest {
            out.start_file(name.as_str(), opts)?;
            out.write_all(&bytes)?;
        }
        Ok(out.finish()?.into_inner())
    }

    /// `presentation.xml` with a new `p:sldIdLst` (and a notes master
    /// list when `notes_master` is new), and its relationships.
    fn presentation_parts(&self, notes_master: Option<&str>) -> anyhow::Result<(String, Vec<Rel>)> {
        let pkg = &self.pkg;
        let restructured = self.structure_changed();
        let slide_rel = |r: &Rel| r.kind() == "slide";
        let live_rids: HashSet<&str> = self
            .slides
            .iter()
            .filter_map(|s| s.origin.as_ref().map(|o| o.rid.as_str()))
            .collect();
        let mut rels: Vec<Rel> = pkg
            .pres_rels
            .iter()
            .filter(|r| !slide_rel(r) || live_rids.contains(r.id.as_str()))
            .cloned()
            .collect();
        let mut list = String::from("<p:sldIdLst>");
        for s in &self.slides {
            match &s.origin {
                Some(o) => list.push_str(&o.sld_id_raw),
                None => {
                    let id = next_rel_id(&rels);
                    list.push_str(&format!("<p:sldId id=\"{}\" r:id=\"{id}\"/>", s.sld_id));
                    rels.push(Rel {
                        id,
                        ty: rel_type("slide"),
                        target: relative(&pkg.pres_part, &s.part),
                        external: false,
                    });
                }
            }
        }
        list.push_str("</p:sldIdLst>");
        if self.slides.is_empty() {
            list.clear();
        }

        let doc = roxmltree::Document::parse(&pkg.pres_xml)?;
        let root = doc.root_element();
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        let mut notes_list = String::new();
        if let Some(m) = notes_master {
            let id = next_rel_id(&rels);
            notes_list = format!(
                "<p:notesMasterIdLst><p:notesMasterId r:id=\"{id}\"/></p:notesMasterIdLst>"
            );
            rels.push(Rel {
                id,
                ty: rel_type("notesMaster"),
                target: relative(&pkg.pres_part, m),
                external: false,
            });
        }
        let masters_end = child(root, NS_P, "sldMasterIdLst")
            .map(|n| n.range().end)
            .context("presentation.xml has no master list")?;
        match child(root, NS_P, "sldIdLst") {
            Some(l) => {
                edits.push((l.range(), list));
                if !notes_list.is_empty() {
                    edits.push((masters_end..masters_end, notes_list));
                }
            }
            None if !notes_list.is_empty() && child(root, NS_P, "handoutMasterIdLst").is_none() => {
                edits.push((masters_end..masters_end, notes_list + &list));
            }
            None => {
                if !notes_list.is_empty() {
                    edits.push((masters_end..masters_end, notes_list));
                }
                let after = ["sldMasterIdLst", "notesMasterIdLst", "handoutMasterIdLst"]
                    .iter()
                    .filter_map(|n| child(root, NS_P, n))
                    .map(|n| n.range().end)
                    .max()
                    .context("presentation.xml has no master list")?;
                edits.push((after..after, list));
            }
        }
        if restructured
            && pkg.has_custom_shows
            && let Some(c) = child(root, NS_P, "custShowLst")
        {
            edits.push((c.range(), String::new()));
        }
        if restructured && pkg.has_sections {
            for d in root
                .descendants()
                .filter(|d| d.tag_name().name() == "sectionLst")
            {
                if let Some(ext) = d.parent().filter(|p| is(*p, NS_P, "ext")) {
                    edits.push((ext.range(), String::new()));
                }
            }
        }
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        let mut xml = pkg.pres_xml.clone();
        for (r, t) in edits {
            xml.replace_range(r, &t);
        }
        // An extension list left empty is invalid.
        let xml = xml.replace("<p:extLst></p:extLst>", "");
        Ok((xml, rels))
    }
}

impl Default for Presentation {
    fn default() -> Self {
        Self::new()
    }
}

fn read_master(
    rd: &mut Reader,
    part: &str,
    table_styles: Option<&str>,
) -> anyhow::Result<(Master, Vec<String>)> {
    let xml = rd.text(part)?.with_context(|| format!("missing {part}"))?;
    let rels = rd.rels(part)?;
    let theme_part = rels
        .iter()
        .find(|r| r.kind() == "theme")
        .and_then(|r| r.part(part));
    let theme = theme_part
        .as_ref()
        .map(|t| rd.text(t))
        .transpose()?
        .flatten()
        .map(|t| Theme::parse(&t))
        .unwrap_or_default();
    let doc = roxmltree::Document::parse(&xml).with_context(|| part.to_owned())?;
    let root = doc.root_element();
    let clr_map = child(root, NS_P, "clrMap")
        .map(ClrMap::parse)
        .unwrap_or_default();
    let theme = Arc::new(theme);
    let colors = ColorCtx {
        theme: &theme,
        map: &clr_map,
        ph: None,
    };
    let styles = child(root, NS_P, "txStyles");
    let style = |name: &str| {
        styles
            .and_then(|s| child(s, NS_P, name))
            .map(|s| ListStyle::parse(s, &colors))
            .unwrap_or_default()
    };
    let cx = ParseCtx {
        src: &xml,
        part,
        rels: &rels,
        colors,
    };
    let csld = child(root, NS_P, "cSld");
    let shapes = csld
        .and_then(|c| child(c, NS_P, "spTree"))
        .map(|t| parse_tree(t, &cx).0)
        .unwrap_or_default();
    let bg = csld.and_then(|c| background(c, &colors));
    let background = csld.and_then(|c| background_fill(c, &colors, part, &rels));
    let layout_parts = child(root, NS_P, "sldLayoutIdLst")
        .map(|l| {
            children(l, NS_P, "sldLayoutId")
                .filter_map(|i| r_attr(i, "id"))
                .filter_map(|rid| rels.iter().find(|r| r.id == rid))
                .filter_map(|r| r.part(part))
                .collect()
        })
        .unwrap_or_default();
    let (title_style, body_style, other_style) =
        (style("titleStyle"), style("bodyStyle"), style("otherStyle"));
    let table_styles = Arc::new(crate::frame::TableStyles::parse(table_styles, &colors));
    Ok((
        Master {
            part: part.to_owned(),
            shapes,
            bg,
            background,
            theme: theme.clone(),
            theme_part,
            clr_map: clr_map.clone(),
            title_style,
            body_style,
            other_style,
            table_styles,
        },
        layout_parts,
    ))
}

fn read_layout(
    rd: &mut Reader,
    part: &str,
    mi: usize,
    master: &Master,
) -> anyhow::Result<Option<Layout>> {
    let Some(xml) = rd.text(part)? else {
        return Ok(None);
    };
    let rels = rd.rels(part)?;
    let Ok(doc) = roxmltree::Document::parse(&xml) else {
        return Ok(None);
    };
    let root = doc.root_element();
    let colors = ColorCtx {
        theme: &master.theme,
        map: &master.clr_map,
        ph: None,
    };
    let cx = ParseCtx {
        src: &xml,
        part,
        rels: &rels,
        colors,
    };
    let csld = child(root, NS_P, "cSld");
    Ok(Some(Layout {
        name: csld
            .and_then(|c| c.attribute("name"))
            .unwrap_or("Layout")
            .to_owned(),
        kind: root.attribute("type").map(str::to_owned),
        part: part.to_owned(),
        master: mi,
        shapes: csld
            .and_then(|c| child(c, NS_P, "spTree"))
            .map(|t| parse_tree(t, &cx).0)
            .unwrap_or_default(),
        bg: csld.and_then(|c| background(c, &colors)),
        background: csld.and_then(|c| background_fill(c, &colors, part, &rels)),
        show_master_sp: attr_bool(root, "showMasterSp").unwrap_or(true),
    }))
}

pub(crate) fn read_slide(
    xml: &str,
    part: &str,
    rels: Vec<Rel>,
    layout: usize,
    master: &Master,
    sld_id: u32,
    origin: Option<Origin>,
) -> Slide {
    let (rels0, origin0) = (rels.clone(), origin.clone());
    let locked_slide = move |why: &str| Slide {
        sld_id,
        part: part.to_owned(),
        layout,
        shapes: Vec::new(),
        bg: None,
        background: None,
        show_master_sp: true,
        hidden: false,
        locked: Some(why.to_owned()),
        rels: Arc::new(rels0.clone()),
        src: Arc::new(SlideSrc {
            prefix: xml.to_owned(),
            suffix: String::new(),
            shapes: Vec::new(),
            rels: rels0.clone(),
        }),
        origin: origin0.clone(),
        notes: None,
    };
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return locked_slide("its XML could not be read");
    };
    let root = doc.root_element();
    let colors = ColorCtx {
        theme: &master.theme,
        map: &master.clr_map,
        ph: None,
    };
    let cx = ParseCtx {
        src: xml,
        part,
        rels: &rels,
        colors,
    };
    let Some(csld) = child(root, NS_P, "cSld") else {
        return locked_slide("it has no shape tree");
    };
    let Some(tree) = child(csld, NS_P, "spTree") else {
        return locked_slide("it has no shape tree");
    };
    let (shapes, range) = parse_tree(tree, &cx);
    let mut locked = None;
    if !usual_prefixes(root) {
        locked = Some("it uses unusual XML namespace prefixes".to_owned());
    }
    let mut prefix = xml[..range.start].to_owned();
    if locked.is_none() {
        ensure_r_decl(&mut prefix, root.range().start);
    }
    let slide = Slide {
        sld_id,
        part: part.to_owned(),
        layout,
        bg: background(csld, &colors),
        background: background_fill(csld, &colors, part, &rels),
        show_master_sp: attr_bool(root, "showMasterSp").unwrap_or(true),
        hidden: attr_bool(root, "show") == Some(false),
        locked: None,
        rels: Arc::new(rels.clone()),
        src: Arc::new(SlideSrc {
            prefix,
            suffix: xml[range.end..].to_owned(),
            shapes: shapes.clone(),
            rels,
        }),
        shapes,
        origin,
        notes: None,
    };
    if let Some(why) = locked {
        let mut l = locked_slide(&why);
        l.shapes = slide.shapes;
        l.bg = slide.bg;
        l.background = slide.background;
        l.hidden = slide.hidden;
        l.show_master_sp = slide.show_master_sp;
        return l;
    }
    slide
}

/// Content type of a media part by its extension.
fn media_type(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("Can't write {}: {}", path.display(), e.error))?;
    Ok(())
}

/// Problems with a package: parts without a content type, and internal
/// relationships whose target doesn't exist. Empty = valid (for tests and
/// as a safety check).
pub fn package_problems(bytes: &[u8]) -> Vec<String> {
    let mut problems = Vec::new();
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return vec!["not a zip".into()];
    };
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    let set: HashSet<&str> = names.iter().map(String::as_str).collect();
    let ct = read_zip_text(&mut zip, "[Content_Types].xml")
        .ok()
        .flatten()
        .map(|x| ContentTypes::parse(&x))
        .unwrap_or_default();
    for n in &names {
        if n == "[Content_Types].xml" || n.ends_with('/') {
            continue;
        }
        if !ct.covers(n) {
            problems.push(format!("{n} has no content type"));
        }
        if let Some(stripped) = n.strip_suffix(".rels") {
            let source = stripped.replacen("_rels/", "", 1);
            let Ok(Some(x)) = read_zip_text(&mut zip, n) else {
                continue;
            };
            for r in parse_rels(&x) {
                if let Some(t) = r.part(&source)
                    && !set.contains(t.as_str())
                {
                    problems.push(format!("{n}: {} → missing {t}", r.id));
                }
            }
        }
    }
    for (p, _) in &ct.overrides {
        if !set.contains(p.as_str()) {
            problems.push(format!("content type override for missing {p}"));
        }
    }
    problems
}
