//! OpenDocument presentation (`.odp`, and `.otp` templates) import,
//! read-only.
//!
//! The package (`content.xml`, `styles.xml`, `Pictures/`) is read and
//! written out as an equivalent `.pptx` package, which
//! [`Presentation::from_bytes`](crate::Presentation::from_bytes) then opens
//! like any other (as [`crate::ppt`] does for PowerPoint 97-2003): masters,
//! layouts, themes and notes follow the `.pptx` rules, and Save writes a new
//! `.pptx` file.
//!
//! What is read: the slide size, the slides in order with their names,
//! master pages (background and the objects on them) as slide masters with
//! their title and outline text styles (sizes, fonts, colours, bullets per
//! level) and four layouts each (Title Slide, Title and Content, Title
//! Only, Blank); each slide's background, hidden slides, the master's
//! objects shown or not, and speaker notes. Objects: title, subtitle and
//! outline placeholders (empty ones too), text frames and shapes with their
//! text (paragraph alignment, indents, spacing, line spacing, bullets and
//! numbering from list styles with their levels; font, size, bold, italic,
//! underline, strike-through, colour, highlight, caps, super- and
//! subscript, letter spacing, links, slide number fields), rectangles,
//! ellipses and arcs, lines and connectors, polygons, polylines and paths,
//! custom shapes (as their DrawingML preset when LibreOffice keeps one,
//! else their own geometry), their place, size, rotation and mirroring,
//! fills (solid, gradients, bitmaps, hatches as patterns, transparency),
//! outlines (colour, width, dashes, arrowheads), shadows, pictures (PNG,
//! JPEG, GIF, EMF, WMF; SVG as a PNG copy with the SVG beside it, as
//! Office writes them), tables (column widths, row heights, merged cells,
//! cell fills, borders, margins and the table's template) and groups. The
//! master's slide number, footer and date frames are put on the slides
//! that show them (with the slide's footer and date declarations). Every
//! text and shape property is resolved through the ODF style chain and
//! written out, so a slide looks the same without the master's help.
//!
//! Left out (and listed in [`Imported::notes`]): charts and other embedded
//! objects without a picture Kantor can show (LibreOffice's replacement
//! pictures are usually StarView metafiles), sounds and videos (a video's
//! preview picture is kept), transitions and animations. Malformed files
//! give an error, never a panic.

mod geom;
mod shape;
mod style;
mod text;

use std::io::{Cursor, Read as _, Write as _};

use anyhow::{Context as _, anyhow, bail};
use roxmltree::{Document, Node, ParsingOptions};

use shape::{Media, Out, Part, Phx, Skipped};
use style::{
    NS_ANIM, NS_DRAW, NS_FO, NS_LOEXT, NS_OFFICE, NS_PRES, NS_SMIL, NS_STYLE, Ref, Styles, emu, esc,
};
use text::{TextOpts, para_props, run_props};

/// File extensions of OpenDocument presentations and templates.
pub const EXTENSIONS: [&str; 2] = ["odp", "otp"];

/// Whether `path` names an OpenDocument presentation (by extension).
pub fn is_odp_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

const MIME: &str = "application/vnd.oasis.opendocument.presentation";

/// Whether `bytes` are an OpenDocument presentation package (its
/// `mimetype`, which ODF stores first and uncompressed).
pub fn is_odp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        && bytes.get(30..38) == Some(b"mimetype")
        && bytes
            .get(38..38 + MIME.len())
            .is_some_and(|m| m == MIME.as_bytes())
}

/// The result of an import: a `.pptx` package and notes about what could
/// not be brought over.
pub struct Imported {
    pub pptx: Vec<u8>,
    pub notes: Vec<String>,
}

fn damaged() -> anyhow::Error {
    anyhow!("it is damaged or not an OpenDocument presentation")
}

const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.presentationml";
const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const GRP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;
const CLR_MAP: &str = r#"<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;

/// LibreOffice's default theme colours: dk1, lt1, dk2, lt2, accent1–6,
/// hlink, folHlink.
const DEFAULT_SCHEME: [&str; 12] = [
    "000000", "FFFFFF", "000000", "FFFFFF", "18A303", "0369A3", "A33E03", "8E03A3", "C99C00",
    "C9211E", "0000EE", "551A8B",
];

/// Layouts written per master: Title Slide, Title and Content, Title Only,
/// Blank.
const LAYOUTS: usize = 4;

/// A read part of the package (at most 256 MB).
fn read_part(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<Option<String>> {
    let mut entry = match zip.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err(damaged()),
    };
    if entry.size() > 256 << 20 {
        bail!("{name} is too large");
    }
    let mut v = Vec::new();
    entry.read_to_end(&mut v).map_err(|_| damaged())?;
    let s = String::from_utf8(v).map_err(|_| damaged())?;
    Ok(Some(s))
}

fn parse(xml: &str) -> anyhow::Result<Document<'_>> {
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .map_err(|e| anyhow!("it is damaged ({e})"))
}

/// Convert an `.odp`/`.otp` file's bytes into a `.pptx` package.
pub fn convert(bytes: &[u8]) -> anyhow::Result<Imported> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| damaged())?;
    if let Some(m) = read_part(&mut zip, "mimetype")? {
        let m = m.trim();
        if !m.is_empty() && !m.starts_with(MIME) {
            bail!("it is not an OpenDocument presentation ({m})");
        }
    }
    if read_part(&mut zip, "META-INF/manifest.xml")?.is_some_and(|m| m.contains("encryption-data"))
    {
        bail!(
            "it is password-protected; Kantor can't open encrypted OpenDocument files. Remove the password in LibreOffice, or save it there as .pptx"
        );
    }
    let content = read_part(&mut zip, "content.xml")?.ok_or_else(damaged)?;
    let styles = read_part(&mut zip, "styles.xml")?.unwrap_or_else(|| {
        "<office:document-styles xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\"/>".into()
    });
    let content = parse(&content)?;
    let styles = parse(&styles)?;
    let pres = content
        .descendants()
        .find(|n| n.has_tag_name((NS_OFFICE, "presentation")))
        .ok_or_else(|| anyhow!("it is not an OpenDocument presentation (no slides inside)"))?;
    let st = Styles::read(&[&styles, &content]);
    let masters: Vec<Node> = styles
        .descendants()
        .chain(content.descendants())
        .filter(|n| n.has_tag_name((NS_STYLE, "master-page")))
        .collect();
    let pages: Vec<Node> = pres
        .children()
        .filter(|n| n.has_tag_name((NS_DRAW, "page")))
        .collect();
    let mut media = Media::new(zip);
    let mut skipped = Skipped::default();
    let w = Writer {
        st: &st,
        masters: &masters,
        pages: &pages,
        pres,
    };
    w.write(&mut media, &mut skipped)
}

struct Writer<'w, 'a, 'i> {
    st: &'w Styles<'a, 'i>,
    masters: &'w [Node<'a, 'i>],
    pages: &'w [Node<'a, 'i>],
    /// `office:presentation` (footer and date declarations).
    pres: Node<'a, 'i>,
}

/// What a master became.
struct OutMaster<'a, 'i> {
    node: Option<Node<'a, 'i>>,
    name: &'a str,
}

fn rels_xml(rels: &[(&str, String, bool)]) -> String {
    let mut s = format!(
        "{DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
    );
    for (i, (ty, target, external)) in rels.iter().enumerate() {
        s.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"{REL}/{ty}\" Target=\"{}\"{}/>",
            i + 1,
            esc(target),
            if *external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn theme_xml(scheme: &[String; 12], major: &str, minor: &str) -> String {
    let names = [
        "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5",
        "accent6", "hlink", "folHlink",
    ];
    let clr: String = names
        .iter()
        .zip(scheme)
        .map(|(n, c)| format!("<a:{n}><a:srgbClr val=\"{c}\"/></a:{n}>"))
        .collect();
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let line = |w: u32| {
        format!(
            "<a:ln w=\"{w}\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:prstDash val=\"solid\"/></a:ln>"
        )
    };
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        "{DECL}<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Imported\"><a:themeElements>\
         <a:clrScheme name=\"Imported\">{clr}</a:clrScheme>\
         <a:fontScheme name=\"Imported\"><a:majorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme>\
         <a:fmtScheme name=\"Imported\"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst>\
         <a:lnStyleLst>{}{}{}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst>\
         <a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements>\
         <a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
        esc(major),
        esc(minor),
        line(6350),
        line(12700),
        line(19050),
    )
}

/// A layout placeholder; `inner` is its `p:spPr` and `p:txBody` (empty
/// for "inherit everything from the master").
fn layout_ph(id: u32, name: &str, ph: &str, inner: &str) -> String {
    let inner = if inner.is_empty() {
        "<p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>"
    } else {
        inner
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr><p:ph {ph}/></p:nvPr></p:nvSpPr>{inner}</p:sp>"
    )
}

/// `a:xfrm` of a frame.
fn xfrm_of(n: Node) -> String {
    let p = shape::Place::of(n);
    format!(
        "<a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
        p.x, p.y, p.w, p.h
    )
}

/// A page size (`style:page-layout`) in EMU.
fn page_size(st: &Styles, layout: Option<&str>) -> Option<(i64, i64)> {
    let pl = st.page_layouts.get(layout?)?;
    let props = pl
        .children()
        .find(|c| c.has_tag_name((NS_STYLE, "page-layout-properties")))?;
    let w = props.attribute((NS_FO, "page-width")).and_then(emu)?;
    let h = props.attribute((NS_FO, "page-height")).and_then(emu)?;
    // 1/100 mm rounding: back to whole points (33.867 cm is 13.333 in).
    let snap = |v: i64| {
        let pt = (v as f64 / 12_700.0).round() as i64 * 12_700;
        if (pt - v).abs() <= 400 { pt } else { v }
    };
    Some((
        snap(w).clamp(914_400, 51_206_400),
        snap(h).clamp(914_400, 51_206_400),
    ))
}

/// The presentation class of an object.
fn class<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attribute((NS_PRES, "class"))
}

fn is_placeholder(n: Node) -> bool {
    n.attribute((NS_PRES, "placeholder")) == Some("true")
}

impl<'a, 'i> Writer<'_, 'a, 'i> {
    fn master_named(&self, name: Option<&str>) -> Option<Node<'a, 'i>> {
        let name = name?;
        self.masters
            .iter()
            .find(|m| m.attribute((NS_STYLE, "name")) == Some(name))
            .copied()
    }

    /// The `p:txStyles` of a master.
    fn tx_styles(&self, m: &str) -> String {
        let st = self.st;
        let title = format!("{m}-title");
        let tchain: Vec<Ref> = vec![("presentation", title.as_str())];
        let title_style = para_props(st, &tchain, None, false, None).xml(
            "lvl1pPr",
            0,
            &with_lang(run_props(st, &tchain)).xml("defRPr", None),
        );
        let outline1 = format!("{m}-outline1");
        let list = st.list_of(&[("presentation", outline1.as_str())]);
        let mut body = String::new();
        for n in 1..=9usize {
            let name = format!("{m}-outline{n}");
            let name = if st.has("presentation", &name) {
                name
            } else {
                outline1.clone()
            };
            let chain: Vec<Ref> = vec![("presentation", name.as_str())];
            let level = list.and_then(|l| text::list_level(l, n));
            let pp = para_props(st, &chain, level, level.is_some(), None);
            body.push_str(&pp.xml(
                &format!("lvl{n}pPr"),
                0,
                &with_lang(run_props(st, &chain)).xml("defRPr", None),
            ));
        }
        let ochain: Vec<Ref> = vec![("graphic", "standard")];
        let other = para_props(st, &ochain, None, false, None).xml(
            "lvl1pPr",
            0,
            &with_lang(run_props(st, &ochain)).xml("defRPr", None),
        );
        format!(
            "<p:txStyles><p:titleStyle>{title_style}</p:titleStyle><p:bodyStyle>{body}</p:bodyStyle><p:otherStyle>{other}</p:otherStyle></p:txStyles>"
        )
    }

    /// The theme colours of a master (LibreOffice keeps them in
    /// `loext:theme`).
    fn scheme(&self, m: Option<Node>) -> [String; 12] {
        let mut s = DEFAULT_SCHEME.map(str::to_owned);
        let names = [
            "dark1",
            "light1",
            "dark2",
            "light2",
            "accent1",
            "accent2",
            "accent3",
            "accent4",
            "accent5",
            "accent6",
            "hyperlink",
            "followed-hyperlink",
        ];
        let Some(theme) = m.and_then(|m| {
            m.descendants()
                .find(|c| c.has_tag_name((NS_LOEXT, "theme-colors")))
        }) else {
            return s;
        };
        for c in theme
            .children()
            .filter(|c| c.has_tag_name((NS_LOEXT, "color")))
        {
            if let (Some(name), Some(col)) = (
                c.attribute((NS_LOEXT, "name")),
                c.attribute((NS_LOEXT, "color")).and_then(style::color),
            ) && let Some(i) = names.iter().position(|n| *n == name)
            {
                s[i] = col;
            }
        }
        s
    }

    fn write(&self, media: &mut Media, skipped: &mut Skipped) -> anyhow::Result<Imported> {
        let st = self.st;
        // Masters: those the slides use, in order, else the first.
        let mut out: Vec<OutMaster> = Vec::new();
        for p in self.pages {
            let name = p.attribute((NS_DRAW, "master-page-name"));
            let node = self.master_named(name).or(self.masters.first().copied());
            let key = node
                .and_then(|n| n.attribute((NS_STYLE, "name")))
                .unwrap_or("");
            if !out.iter().any(|o| o.name == key) {
                out.push(OutMaster { node, name: key });
            }
        }
        if out.is_empty() {
            let node = self.masters.first().copied();
            out.push(OutMaster {
                node,
                name: node
                    .and_then(|n| n.attribute((NS_STYLE, "name")))
                    .unwrap_or(""),
            });
        }
        let slide_size = page_size(
            st,
            out[0]
                .node
                .and_then(|n| n.attribute((NS_STYLE, "page-layout-name"))),
        )
        .unwrap_or((10_080_000, 5_670_000));
        let notes_size = out[0]
            .node
            .and_then(|n| {
                n.children()
                    .find(|c| c.has_tag_name((NS_PRES, "notes")))
                    .and_then(|c| c.attribute((NS_STYLE, "page-layout-name")))
            })
            .and_then(|l| page_size(st, Some(l)))
            .unwrap_or((6_858_000, 9_144_000));

        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        let mut overrides: Vec<(String, String)> = Vec::new();
        let mut pres_rels: Vec<(&str, String, bool)> = Vec::new();
        let mut master_ids = String::new();
        let mut next_layout_id: u64 = 2_147_483_648 + out.len() as u64;

        for (mi, m) in out.iter().enumerate() {
            let n = mi + 1;
            let scheme = self.scheme(m.node);
            let font_of = |sty: String| {
                run_props(st, &[("presentation", sty.as_str())])
                    .latin
                    .unwrap_or_else(|| "Liberation Sans".into())
            };
            parts.push((
                format!("ppt/theme/theme{n}.xml"),
                theme_xml(
                    &scheme,
                    &font_of(format!("{}-title", m.name)),
                    &font_of(format!("{}-outline1", m.name)),
                )
                .into_bytes(),
            ));
            overrides.push((
                format!("/ppt/theme/theme{n}.xml"),
                "application/vnd.openxmlformats-officedocument.theme+xml".into(),
            ));

            let mut part = Part {
                st,
                media,
                skipped,
                rels: Vec::new(),
                next_id: 1,
                master: m.name.to_owned(),
                fld: 0,
                footer: None,
                slide_no: 0,
                date: None,
            };
            for l in 0..LAYOUTS {
                part.rel(
                    "slideLayout",
                    format!("../slideLayouts/slideLayout{}.xml", mi * LAYOUTS + l + 1),
                    false,
                );
            }
            part.rel("theme", format!("../theme/theme{n}.xml"), false);
            let bg = m
                .node
                .and_then(|node| node.attribute((NS_DRAW, "style-name")))
                .and_then(|dp| part.fill(&[("drawing-page", dp)], "drawing-page-properties"))
                .filter(|f| f != "<a:noFill/>")
                .unwrap_or_else(|| "<a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill>".into());
            let mut title_pos = None;
            let mut body_pos = None;
            let shapes: Vec<Out> = match m.node {
                Some(node) => {
                    let mut next = 0u32;
                    let mut ph = |o: Node| -> Phx {
                        match class(o) {
                            Some("title") => {
                                title_pos = Some(xfrm_of(o));
                                Phx {
                                    ph: "<p:ph type=\"title\"/>".into(),
                                    ..Phx::default()
                                }
                            }
                            Some("outline") => {
                                body_pos = Some(xfrm_of(o));
                                Phx {
                                    ph: "<p:ph type=\"body\" idx=\"1\"/>".into(),
                                    outline: true,
                                    ..Phx::default()
                                }
                            }
                            Some("date-time") => Phx {
                                ph: "<p:ph type=\"dt\" sz=\"half\" idx=\"10\"/>".into(),
                                ..Phx::default()
                            },
                            Some("footer") => Phx {
                                ph: "<p:ph type=\"ftr\" sz=\"quarter\" idx=\"11\"/>".into(),
                                ..Phx::default()
                            },
                            Some("page-number") => Phx {
                                ph: "<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>".into(),
                                ..Phx::default()
                            },
                            Some(c)
                                if is_placeholder(o)
                                    || matches!(
                                        c,
                                        "notes" | "header" | "page" | "handout" | "subtitle"
                                    ) =>
                            {
                                next += 1;
                                Phx {
                                    skip: true,
                                    ..Phx::default()
                                }
                            }
                            _ => Phx::default(),
                        }
                    };
                    part.shapes(node, &mut ph)
                }
                None => Vec::new(),
            };
            let shapes_xml: String = shapes.into_iter().map(|s| s.xml).collect();
            let mut layout_ids = String::new();
            for l in 0..LAYOUTS {
                layout_ids.push_str(&format!(
                    "<p:sldLayoutId id=\"{next_layout_id}\" r:id=\"rId{}\"/>",
                    l + 1
                ));
                next_layout_id += 1;
            }
            let master_xml = format!(
                "{DECL}<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr>{bg}<a:effectLst/></p:bgPr></p:bg><p:spTree>{GRP}{shapes_xml}</p:spTree></p:cSld>{CLR_MAP}\
                 <p:sldLayoutIdLst>{layout_ids}</p:sldLayoutIdLst>{}</p:sldMaster>",
                self.tx_styles(m.name)
            );
            let mrels = std::mem::take(&mut part.rels);
            drop(part);
            parts.push((
                format!("ppt/slideMasters/slideMaster{n}.xml"),
                master_xml.into_bytes(),
            ));
            parts.push((
                format!("ppt/slideMasters/_rels/slideMaster{n}.xml.rels"),
                rels_xml(&mrels).into_bytes(),
            ));
            overrides.push((
                format!("/ppt/slideMasters/slideMaster{n}.xml"),
                format!("{CT}.slideMaster+xml"),
            ));
            pres_rels.push((
                "slideMaster",
                format!("slideMasters/slideMaster{n}.xml"),
                false,
            ));
            master_ids.push_str(&format!(
                "<p:sldMasterId id=\"{}\" r:id=\"rId{}\"/>",
                2_147_483_648u64 + mi as u64,
                pres_rels.len()
            ));

            // Layouts.
            let sub = format!("{}-subtitle", m.name);
            let schain: Vec<Ref> = vec![("presentation", sub.as_str())];
            let sub_style = para_props(st, &schain, None, false, None).xml(
                "lvl1pPr",
                0,
                &with_lang(run_props(st, &schain)).xml("defRPr", None),
            );
            for l in 0..LAYOUTS {
                let ln = mi * LAYOUTS + l + 1;
                let title = |ty: &str| {
                    layout_ph(
                        2,
                        "Title 1",
                        &format!("type=\"{ty}\""),
                        &title_pos
                            .as_ref()
                            .map(|x| {
                                format!(
                                    "<p:spPr>{x}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>"
                                )
                            })
                            .unwrap_or_default(),
                    )
                };
                let (name, ty, body) = match l {
                    0 => (
                        "Title Slide",
                        "title",
                        title("ctrTitle")
                            + &layout_ph(
                                3,
                                "Subtitle 2",
                                "type=\"subTitle\" idx=\"1\"",
                                &format!(
                                    "<p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle>{sub_style}</a:lstStyle><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody>",
                                    body_pos.clone().unwrap_or_default()
                                ),
                            ),
                    ),
                    1 => (
                        "Title and Content",
                        "obj",
                        title("title") + &layout_ph(3, "Content Placeholder 2", "idx=\"1\"", ""),
                    ),
                    2 => ("Title Only", "titleOnly", title("title")),
                    _ => ("Blank", "blank", String::new()),
                };
                let xml = format!(
                    "{DECL}<p:sldLayout {NS} type=\"{ty}\" preserve=\"1\"><p:cSld name=\"{name}\"><p:spTree>{GRP}{body}</p:spTree></p:cSld>\
                     <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
                );
                parts.push((
                    format!("ppt/slideLayouts/slideLayout{ln}.xml"),
                    xml.into_bytes(),
                ));
                parts.push((
                    format!("ppt/slideLayouts/_rels/slideLayout{ln}.xml.rels"),
                    rels_xml(&[(
                        "slideMaster",
                        format!("../slideMasters/slideMaster{n}.xml"),
                        false,
                    )])
                    .into_bytes(),
                ));
                overrides.push((
                    format!("/ppt/slideLayouts/slideLayout{ln}.xml"),
                    format!("{CT}.slideLayout+xml"),
                ));
            }
        }

        // Speaker notes of each page.
        let notes_of = |p: &Node<'a, 'i>| -> Option<Node<'a, 'i>> {
            let notes = p.children().find(|c| c.has_tag_name((NS_PRES, "notes")))?;
            notes
                .descendants()
                .find(|c| c.has_tag_name((NS_DRAW, "frame")) && class(*c) == Some("notes"))
                .filter(|f| {
                    f.descendants()
                        .any(|d| d.is_text() && !d.text().unwrap_or_default().trim().is_empty())
                })
        };
        let has_notes = self.pages.iter().any(|p| notes_of(p).is_some());
        let mut notes_master_rel = None;
        if has_notes {
            let tn = out.len() + 1;
            parts.push((
                format!("ppt/theme/theme{tn}.xml"),
                theme_xml(
                    &self.scheme(out[0].node),
                    "Liberation Sans",
                    "Liberation Sans",
                )
                .into_bytes(),
            ));
            overrides.push((
                format!("/ppt/theme/theme{tn}.xml"),
                "application/vnd.openxmlformats-officedocument.theme+xml".into(),
            ));
            parts.push((
                "ppt/notesMasters/notesMaster1.xml".into(),
                crate::template::notes_master().into_bytes(),
            ));
            parts.push((
                "ppt/notesMasters/_rels/notesMaster1.xml.rels".into(),
                rels_xml(&[("theme", format!("../theme/theme{tn}.xml"), false)]).into_bytes(),
            ));
            overrides.push((
                "/ppt/notesMasters/notesMaster1.xml".into(),
                format!("{CT}.notesMaster+xml"),
            ));
            pres_rels.push(("notesMaster", "notesMasters/notesMaster1.xml".into(), false));
            notes_master_rel = Some(pres_rels.len());
        }

        // Slides.
        let mut slide_ids = String::new();
        let mut notes_count = 0usize;
        let mut animated = false;
        for (si, p) in self.pages.iter().enumerate() {
            let n = si + 1;
            let mname = p.attribute((NS_DRAW, "master-page-name"));
            let mnode = self.master_named(mname).or(self.masters.first().copied());
            let key = mnode
                .and_then(|x| x.attribute((NS_STYLE, "name")))
                .unwrap_or("");
            let mi = out.iter().position(|o| o.name == key).unwrap_or(0);
            // The layout: by the placeholders on the slide.
            let classes: Vec<&str> = p
                .children()
                .filter(|c| c.is_element())
                .filter_map(class)
                .collect();
            let layout = if classes.contains(&"subtitle") {
                0
            } else if classes.iter().any(|c| {
                matches!(
                    *c,
                    "outline" | "text" | "object" | "graphic" | "chart" | "table"
                )
            }) {
                1
            } else if classes.contains(&"title") {
                2
            } else {
                3
            };
            let dp = p.attribute((NS_DRAW, "style-name"));
            let dchain: Vec<Ref> = dp.map(|d| ("drawing-page", d)).into_iter().collect();
            let dprop = |ns: &str, a: &str| st.prop(&dchain, "drawing-page-properties", ns, a);
            animated |= p
                .children()
                .any(|c| c.tag_name().namespace() == Some(NS_ANIM))
                || dprop(NS_PRES, "transition-type").is_some_and(|t| t != "none")
                || dprop(NS_SMIL, "type").is_some()
                || dprop(NS_PRES, "transition-style").is_some_and(|t| t != "none");
            let hidden = dprop(NS_PRES, "visibility") == Some("hidden");
            let bg_visible = dprop(NS_PRES, "background-visible") != Some("false");
            let objects_visible = dprop(NS_PRES, "background-objects-visible") != Some("false");
            let mut part = Part {
                st,
                media,
                skipped,
                rels: vec![(
                    "slideLayout",
                    format!(
                        "../slideLayouts/slideLayout{}.xml",
                        mi * LAYOUTS + layout + 1
                    ),
                    false,
                )],
                next_id: 1,
                master: key.to_owned(),
                fld: 0,
                footer: None,
                slide_no: n,
                date: None,
            };
            let own_bg = part
                .fill(&dchain, "drawing-page-properties")
                .filter(|f| f != "<a:noFill/>");
            let bg = match own_bg {
                Some(f) => format!("<p:bg><p:bgPr>{f}<a:effectLst/></p:bgPr></p:bg>"),
                None if !bg_visible => "<p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>".into(),
                None => String::new(),
            };
            let mut next_idx = 0u32;
            let mut ph = |o: Node| -> Phx {
                let Some(c) = class(o) else {
                    return Phx::default();
                };
                let empty = is_placeholder(o);
                match c {
                    "title" => Phx {
                        ph: if layout == 0 {
                            "<p:ph type=\"ctrTitle\"/>".into()
                        } else {
                            "<p:ph type=\"title\"/>".into()
                        },
                        ..Phx::default()
                    },
                    "subtitle" => {
                        next_idx += 1;
                        Phx {
                            ph: format!("<p:ph type=\"subTitle\" idx=\"{next_idx}\"/>"),
                            ..Phx::default()
                        }
                    }
                    "outline" | "text" => {
                        next_idx += 1;
                        Phx {
                            ph: if next_idx == 1 && c == "outline" {
                                "<p:ph idx=\"1\"/>".into()
                            } else {
                                format!("<p:ph type=\"body\" idx=\"{next_idx}\"/>")
                            },
                            outline: c == "outline",
                            ..Phx::default()
                        }
                    }
                    _ if empty => Phx {
                        skip: true,
                        ..Phx::default()
                    },
                    "notes" | "page" | "handout" | "header" | "date-time" | "page-number" => Phx {
                        skip: c != "page-number" || empty,
                        ..Phx::default()
                    },
                    _ => Phx::default(),
                }
            };
            let shapes: Vec<Out> = p
                .children()
                .filter(|c| c.is_element() && !c.has_tag_name((NS_PRES, "notes")))
                .filter_map(|c| part.object(c, &mut ph))
                .collect();
            let mut shapes_xml: String = shapes.into_iter().map(|s| s.xml).collect();
            // The master's slide number, footer and date, when the slide
            // shows them.
            let decl = |kind: &str, attr: &str| {
                let name = p.attribute((NS_PRES, attr))?;
                self.pres.children().find(|d| {
                    d.has_tag_name((NS_PRES, kind)) && d.attribute((NS_PRES, "name")) == Some(name)
                })
            };
            part.footer = decl("footer-decl", "use-footer-name")
                .map(|d| d.text().unwrap_or_default().trim().to_owned());
            part.date = decl("date-time-decl", "use-date-time-name").map(|d| {
                if d.attribute((NS_PRES, "source")) == Some("current-date") {
                    Err(())
                } else {
                    Ok(d.text().unwrap_or_default().trim().to_owned())
                }
            });
            for (shown, cls, ph_xml) in [
                (
                    "display-page-number",
                    "page-number",
                    "<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>",
                ),
                (
                    "display-footer",
                    "footer",
                    "<p:ph type=\"ftr\" sz=\"quarter\" idx=\"11\"/>",
                ),
                (
                    "display-date-time",
                    "date-time",
                    "<p:ph type=\"dt\" sz=\"half\" idx=\"10\"/>",
                ),
            ] {
                if dprop(NS_PRES, shown) != Some("true") || !objects_visible {
                    continue;
                }
                let Some(frame) = mnode.and_then(|m| {
                    m.children()
                        .find(|c| c.has_tag_name((NS_DRAW, "frame")) && class(*c) == Some(cls))
                }) else {
                    continue;
                };
                let mut ph = |_: Node| Phx {
                    ph: ph_xml.into(),
                    ..Phx::default()
                };
                if let Some(o) = part.object(frame, &mut ph)
                    && (o.xml.contains("<a:r>") || o.xml.contains("<a:fld"))
                {
                    shapes_xml.push_str(&o.xml);
                }
            }
            if let Some(frame) = notes_of(p) {
                notes_count += 1;
                let nn = notes_count;
                part.rel(
                    "notesSlide",
                    format!("../notesSlides/notesSlide{nn}.xml"),
                    false,
                );
                let chain = Part::chain_of(frame);
                let box_ = frame
                    .children()
                    .find(|c| c.has_tag_name((NS_DRAW, "text-box")))
                    .unwrap_or(frame);
                let paras = part.paragraphs(
                    box_,
                    &TextOpts {
                        base: &chain,
                        outline: false,
                        notes: true,
                    },
                );
                let body = if paras.is_empty() {
                    "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>".to_owned()
                } else {
                    paras.concat()
                };
                let xml = crate::template::notes_slide()
                    .replace("<a:p><a:endParaRPr lang=\"en-US\"/></a:p>", &body);
                parts.push((
                    format!("ppt/notesSlides/notesSlide{nn}.xml"),
                    xml.into_bytes(),
                ));
                parts.push((
                    format!("ppt/notesSlides/_rels/notesSlide{nn}.xml.rels"),
                    rels_xml(&[
                        (
                            "notesMaster",
                            "../notesMasters/notesMaster1.xml".into(),
                            false,
                        ),
                        ("slide", format!("../slides/slide{n}.xml"), false),
                    ])
                    .into_bytes(),
                ));
                overrides.push((
                    format!("/ppt/notesSlides/notesSlide{nn}.xml"),
                    format!("{CT}.notesSlide+xml"),
                ));
            }
            let mut attrs = String::new();
            if !objects_visible {
                attrs.push_str(" showMasterSp=\"0\"");
            }
            if hidden {
                attrs.push_str(" show=\"0\"");
            }
            let name = p
                .attribute((NS_DRAW, "name"))
                .filter(|nm| !is_default_page_name(nm))
                .map(|nm| format!(" name=\"{}\"", esc(nm)))
                .unwrap_or_default();
            let xml = format!(
                "{DECL}<p:sld {NS}{attrs}><p:cSld{name}>{bg}<p:spTree>{GRP}{shapes_xml}</p:spTree></p:cSld>\
                 <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
            );
            let srels = std::mem::take(&mut part.rels);
            drop(part);
            parts.push((format!("ppt/slides/slide{n}.xml"), xml.into_bytes()));
            parts.push((
                format!("ppt/slides/_rels/slide{n}.xml.rels"),
                rels_xml(&srels).into_bytes(),
            ));
            overrides.push((
                format!("/ppt/slides/slide{n}.xml"),
                format!("{CT}.slide+xml"),
            ));
            pres_rels.push(("slide", format!("slides/slide{n}.xml"), false));
            slide_ids.push_str(&format!(
                "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
                256 + si,
                pres_rels.len()
            ));
        }

        // Presentation.
        let dchain: Vec<Ref> = vec![("graphic", "standard")];
        let default_style = para_props(st, &dchain, None, false, None).xml(
            "lvl1pPr",
            0,
            &with_lang(run_props(st, &dchain)).xml("defRPr", None),
        );
        for (ty, target) in [
            ("presProps", "presProps.xml"),
            ("viewProps", "viewProps.xml"),
            ("tableStyles", "tableStyles.xml"),
        ] {
            pres_rels.push((ty, target.into(), false));
        }
        let notes_ids = notes_master_rel
            .map(|r| {
                format!(
                    "<p:notesMasterIdLst><p:notesMasterId r:id=\"rId{r}\"/></p:notesMasterIdLst>"
                )
            })
            .unwrap_or_default();
        let sld_list = if slide_ids.is_empty() {
            String::new()
        } else {
            format!("<p:sldIdLst>{slide_ids}</p:sldIdLst>")
        };
        let presentation = format!(
            "{DECL}<p:presentation {NS} saveSubsetFonts=\"1\"><p:sldMasterIdLst>{master_ids}</p:sldMasterIdLst>{notes_ids}{sld_list}\
             <p:sldSz cx=\"{}\" cy=\"{}\"/><p:notesSz cx=\"{}\" cy=\"{}\"/>\
             <p:defaultTextStyle><a:defPPr><a:defRPr lang=\"en-US\"/></a:defPPr>{default_style}</p:defaultTextStyle></p:presentation>",
            slide_size.0, slide_size.1, notes_size.0, notes_size.1
        );
        parts.push(("ppt/presentation.xml".into(), presentation.into_bytes()));
        parts.push((
            "ppt/_rels/presentation.xml.rels".into(),
            rels_xml(&pres_rels).into_bytes(),
        ));
        parts.push((
            "ppt/presProps.xml".into(),
            format!("{DECL}<p:presentationPr {NS}/>").into_bytes(),
        ));
        parts.push((
            "ppt/viewProps.xml".into(),
            format!("{DECL}<p:viewPr {NS}><p:gridSpacing cx=\"76200\" cy=\"76200\"/></p:viewPr>")
                .into_bytes(),
        ));
        parts.push((
            "ppt/tableStyles.xml".into(),
            format!(
                "{DECL}<a:tblStyleLst xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" def=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\"/>"
            )
            .into_bytes(),
        ));
        for (name, data) in std::mem::take(&mut media.parts) {
            parts.push((format!("ppt/media/{name}"), data));
        }
        let mut ct = format!(
            "{DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Default Extension=\"png\" ContentType=\"image/png\"/>\
             <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
             <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
             <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
             <Default Extension=\"svg\" ContentType=\"image/svg+xml\"/>\
             <Default Extension=\"emf\" ContentType=\"image/x-emf\"/>\
             <Default Extension=\"wmf\" ContentType=\"image/x-wmf\"/>\
             <Default Extension=\"pict\" ContentType=\"image/pict\"/>\
             <Override PartName=\"/ppt/presentation.xml\" ContentType=\"{CT}.presentation.main+xml\"/>\
             <Override PartName=\"/ppt/presProps.xml\" ContentType=\"{CT}.presProps+xml\"/>\
             <Override PartName=\"/ppt/viewProps.xml\" ContentType=\"{CT}.viewProps+xml\"/>\
             <Override PartName=\"/ppt/tableStyles.xml\" ContentType=\"{CT}.tableStyles+xml\"/>\
             <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
             <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>"
        );
        for (name, ty) in &overrides {
            ct.push_str(&format!(
                "<Override PartName=\"{name}\" ContentType=\"{ty}\"/>"
            ));
        }
        ct.push_str("</Types>");
        parts.push(("[Content_Types].xml".into(), ct.into_bytes()));
        parts.push((
            "_rels/.rels".into(),
            format!(
                "{DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
                 <Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"ppt/presentation.xml\"/>\
                 <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
                 <Relationship Id=\"rId3\" Type=\"{REL}/extended-properties\" Target=\"docProps/app.xml\"/></Relationships>"
            )
            .into_bytes(),
        ));
        parts.push((
            "docProps/core.xml".into(),
            format!(
                "{DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
                 xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
                 xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title></dc:title></cp:coreProperties>"
            )
            .into_bytes(),
        ));
        parts.push((
            "docProps/app.xml".into(),
            format!(
                "{DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>Kantor</Application></Properties>"
            )
            .into_bytes(),
        ));

        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in parts {
            zip.start_file(name, opts).context("writing the package")?;
            zip.write_all(&data).context("writing the package")?;
        }
        let pptx = zip.finish().context("writing the package")?.into_inner();

        let mut notes = Vec::new();
        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {many}")
            }
        };
        if skipped.charts > 0 {
            notes.push(format!(
                "{} left out",
                plural(skipped.charts, "chart", "charts")
            ));
        }
        if skipped.objects > 0 {
            notes.push(format!(
                "{} left out",
                plural(skipped.objects, "embedded object", "embedded objects")
            ));
        }
        if skipped.media > 0 {
            notes.push(format!(
                "{} left out",
                plural(skipped.media, "video or sound", "videos and sounds")
            ));
        }
        if skipped.pictures > 0 {
            notes.push(format!(
                "{} in a format Kantor can't show left out",
                plural(skipped.pictures, "picture", "pictures")
            ));
        }
        if skipped.shapes > 0 {
            notes.push(format!(
                "{} Kantor can't read left out",
                plural(skipped.shapes, "shape", "shapes")
            ));
        }
        if animated {
            notes.push("slide transitions and animations were left out".into());
        }
        Ok(Imported { pptx, notes })
    }
}

/// LibreOffice's automatic slide names (`page1`, `Slide 2`).
fn is_default_page_name(n: &str) -> bool {
    let n = n.trim();
    ["page", "Slide ", "Folie "].iter().any(|p| {
        n.strip_prefix(p)
            .is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()))
    })
}

fn with_lang(mut r: text::RunProps) -> text::RunProps {
    r.lang.get_or_insert_with(|| "en-US".into());
    r
}
