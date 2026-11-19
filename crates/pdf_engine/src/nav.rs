//! Bookmarks (the document outline), link annotations and destinations.

use std::collections::{HashMap, HashSet};

use hayro::hayro_interpret::TransformExt as _;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier, Rect};
use hayro::kurbo::Point;

use crate::PdfDoc;

/// Deepest outline nesting read (guards against malformed files).
const MAX_DEPTH: usize = 32;
/// Most outline items read (likewise).
const MAX_ITEMS: usize = 20_000;

/// A place in the document: a page, and where on it (page space).
#[derive(Clone, Debug, PartialEq)]
pub struct Dest {
    pub page: usize,
    /// The point to show at the top left, when the destination names one.
    pub x: Option<f32>,
    pub y: Option<f32>,
}

/// Where a link or bookmark goes.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// A place in this document.
    Dest(Dest),
    /// An address (web page, e-mail…) opened outside Kantor.
    Uri(String),
}

/// A link annotation on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// `[x0, y0, x1, y1]`, page space.
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

/// A bookmark.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    pub target: Option<LinkTarget>,
    /// Shown expanded when the file opens (a positive `/Count`).
    pub open: bool,
    pub children: Vec<OutlineItem>,
}

impl PdfDoc {
    fn catalog(&self) -> Option<Dict<'_>> {
        let xref = self.pdf().xref();
        xref.get::<Dict<'_>>(xref.root_id())
    }

    /// The document's bookmarks.
    pub fn outline(&self) -> Vec<OutlineItem> {
        let Some(first) = self
            .catalog()
            .and_then(|c| c.get::<Dict<'_>>("Outlines"))
            .and_then(|o| o.get::<Dict<'_>>("First"))
        else {
            return Vec::new();
        };
        let names = NamedDests::new(self);
        let mut seen = HashSet::new();
        let mut budget = MAX_ITEMS;
        self.outline_level(first, &names, &mut seen, &mut budget, 0)
    }

    fn outline_level(
        &self,
        first: Dict<'_>,
        names: &NamedDests<'_>,
        seen: &mut HashSet<ObjectIdentifier>,
        budget: &mut usize,
        depth: usize,
    ) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        let mut next = Some(first);
        while let Some(item) = next {
            if *budget == 0 || item.obj_id().is_some_and(|id| !seen.insert(id)) {
                break;
            }
            *budget -= 1;
            let title = item
                .get::<hayro::hayro_syntax::object::String<'_>>("Title")
                .map(|s| decode_text(s.as_bytes()))
                .unwrap_or_default();
            let target = self.target_of(&item, names);
            let children = match item.get::<Dict<'_>>("First") {
                Some(child) if depth < MAX_DEPTH => {
                    self.outline_level(child, names, seen, budget, depth + 1)
                }
                _ => Vec::new(),
            };
            items.push(OutlineItem {
                title: clean_title(&title),
                target,
                open: item.get::<i64>("Count").is_some_and(|c| c > 0),
                children,
            });
            next = item.get::<Dict<'_>>("Next");
        }
        items
    }

    /// The links on page `page`.
    pub fn links(&self, page: usize) -> Vec<Link> {
        let Some(p) = self.pdf().pages().get(page) else {
            return Vec::new();
        };
        let Some(annots) = p.raw().get::<Array<'_>>("Annots") else {
            return Vec::new();
        };
        let to_page = p.initial_transform(true).to_kurbo();
        let names = NamedDests::new(self);
        let mut links = Vec::new();
        for annot in annots.iter::<Dict<'_>>() {
            if annot.get::<Name<'_>>("Subtype").as_deref() != Some(b"Link".as_slice()) {
                continue;
            }
            let Some(r) = annot.get::<Rect>("Rect") else {
                continue;
            };
            let Some(target) = self.target_of(&annot, &names) else {
                continue;
            };
            let a = to_page * Point::new(r.x0, r.y0);
            let b = to_page * Point::new(r.x1, r.y1);
            links.push(Link {
                rect: [
                    a.x.min(b.x) as f32,
                    a.y.min(b.y) as f32,
                    a.x.max(b.x) as f32,
                    a.y.max(b.y) as f32,
                ],
                target,
            });
        }
        links
    }

    /// The target of an outline item or link: `/Dest`, or a GoTo / URI
    /// action in `/A`.
    fn target_of(&self, item: &Dict<'_>, names: &NamedDests<'_>) -> Option<LinkTarget> {
        if let Some(dest) = item.get::<Object<'_>>("Dest") {
            return self.dest(dest, names, 0).map(LinkTarget::Dest);
        }
        let action = item.get::<Dict<'_>>("A")?;
        match action.get::<Name<'_>>("S")?.as_str() {
            "GoTo" => self
                .dest(action.get::<Object<'_>>("D")?, names, 0)
                .map(LinkTarget::Dest),
            "URI" => {
                let uri = action.get::<hayro::hayro_syntax::object::String<'_>>("URI")?;
                let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_owned();
                (!uri.is_empty()).then_some(LinkTarget::Uri(uri))
            }
            _ => None,
        }
    }

    /// A destination: an explicit `[page /XYZ left top zoom]`-style array,
    /// a name looked up in the document, or a dictionary holding one in `/D`.
    fn dest(&self, obj: Object<'_>, names: &NamedDests<'_>, depth: u8) -> Option<Dest> {
        if depth > 4 {
            return None;
        }
        match obj {
            Object::Array(a) => self.explicit_dest(&a),
            Object::Dict(d) => self.dest(d.get::<Object<'_>>("D")?, names, depth + 1),
            Object::Name(n) => self.dest(names.get(n.as_ref())?, names, depth + 1),
            Object::String(s) => self.dest(names.get(s.as_bytes())?, names, depth + 1),
            _ => None,
        }
    }

    fn explicit_dest(&self, a: &Array<'_>) -> Option<Dest> {
        let mut items = a.raw_iter();
        let page = match items.next()? {
            MaybeRef::Ref(r) => self.page_index(r.into())?,
            MaybeRef::NotRef(Object::Number(n)) => usize::try_from(n.as_i64()).ok()?,
            MaybeRef::NotRef(Object::Dict(d)) => self.page_index(d.obj_id()?)?,
            _ => return None,
        };
        let p = self.pdf().pages().get(page)?;
        let kind = match items.next() {
            Some(MaybeRef::NotRef(Object::Name(n))) => n.as_str().to_owned(),
            _ => String::new(),
        };
        let mut num = || match items.next() {
            Some(MaybeRef::NotRef(Object::Number(n))) => Some(n.as_f64()),
            _ => None,
        };
        let (left, top) = match kind.as_str() {
            "XYZ" => (num(), num()),
            "FitH" | "FitBH" => (None, num()),
            "FitV" | "FitBV" => (num(), None),
            "FitR" => {
                let (l, _b, _r, t) = (num(), num(), num(), num());
                (l, t)
            }
            _ => (None, None),
        };
        let to_page = p.initial_transform(true).to_kurbo();
        let at = to_page * Point::new(left.unwrap_or(0.0), top.unwrap_or(0.0));
        Some(Dest {
            page,
            x: left.map(|_| at.x as f32),
            y: top.map(|_| at.y as f32),
        })
    }
}

/// Named destinations: the catalog's `/Dests` dictionary (PDF 1.1) and the
/// `/Names /Dests` name tree.
struct NamedDests<'a> {
    old: Option<Dict<'a>>,
    tree: HashMap<Vec<u8>, Object<'a>>,
}

impl<'a> NamedDests<'a> {
    fn new(doc: &'a PdfDoc) -> Self {
        let catalog = doc.catalog();
        let old = catalog.as_ref().and_then(|c| c.get::<Dict<'a>>("Dests"));
        let mut tree = HashMap::new();
        if let Some(root) = catalog
            .and_then(|c| c.get::<Dict<'a>>("Names"))
            .and_then(|n| n.get::<Dict<'a>>("Dests"))
        {
            let mut seen = HashSet::new();
            collect_names(root, &mut tree, &mut seen, 0);
        }
        Self { old, tree }
    }

    fn get(&self, name: &[u8]) -> Option<Object<'a>> {
        if let Some(o) = self.tree.get(name) {
            return Some(o.clone());
        }
        self.old.as_ref()?.get::<Object<'a>>(name)
    }
}

fn collect_names<'a>(
    node: Dict<'a>,
    out: &mut HashMap<Vec<u8>, Object<'a>>,
    seen: &mut HashSet<ObjectIdentifier>,
    depth: usize,
) {
    if depth > MAX_DEPTH || node.obj_id().is_some_and(|id| !seen.insert(id)) {
        return;
    }
    if let Some(names) = node.get::<Array<'a>>("Names") {
        let mut it = names.iter::<Object<'a>>();
        while let (Some(key), Some(value)) = (it.next(), it.next()) {
            let key = match key {
                Object::String(s) => s.as_bytes().to_vec(),
                Object::Name(n) => n.as_ref().to_vec(),
                _ => continue,
            };
            out.entry(key).or_insert(value);
        }
    }
    if let Some(kids) = node.get::<Array<'a>>("Kids") {
        for kid in kids.iter::<Dict<'a>>() {
            collect_names(kid, out, seen, depth + 1);
        }
    }
}

/// A PDF text string: UTF-16BE or UTF-8 with a byte-order mark, else
/// PDFDocEncoding.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes.iter().map(|&b| pdf_doc_char(b)).collect()
}

/// PDFDocEncoding: Latin-1 except 0x18–0x1F and 0x80–0x9F.
fn pdf_doc_char(b: u8) -> char {
    const HIGH: [char; 32] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{FFFD}',
    ];
    const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    match b {
        0x18..=0x1F => LOW[usize::from(b - 0x18)],
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// A bookmark title on one line (some files put line breaks in them).
fn clean_title(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
