//! A minimal, valid new presentation: 16:9, one master with the Office
//! theme, two layouts ("Title Slide", "Title and Content") and one title
//! slide. Built as a zip in memory and then opened like any file.

use std::io::{Cursor, Write as _};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.presentationml";

const GRP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

/// Start of a new slide's XML, through the group properties of its shape tree.
pub(crate) fn slide_prefix() -> String {
    format!("{DECL}<p:sld {NS}><p:cSld><p:spTree>{GRP}")
}

/// End of a new slide's XML, after the shapes.
pub(crate) const SLIDE_SUFFIX: &str =
    "</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>";

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = format!(
        "{DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
    );
    for (id, ty, target) in items {
        let ty = if ty.starts_with("http") {
            (*ty).to_owned()
        } else {
            format!("{R}/{ty}")
        };
        s.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{ty}\" Target=\"{target}\"/>"
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn ph(
    id: u32,
    name: &str,
    ty: &str,
    idx: Option<u32>,
    xfrm: Option<[i64; 4]>,
    body: &str,
) -> String {
    let idx = idx.map(|i| format!(" idx=\"{i}\"")).unwrap_or_default();
    let ty = if ty.is_empty() {
        String::new()
    } else {
        format!(" type=\"{ty}\"")
    };
    let xfrm = xfrm
        .map(|[x, y, cx, cy]| {
            format!("<a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>")
        })
        .unwrap_or_default();
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>\
         <p:nvPr><p:ph{ty}{idx}/></p:nvPr></p:nvSpPr><p:spPr>{xfrm}</p:spPr><p:txBody>{body}</p:txBody></p:sp>"
    )
}

fn level(n: u8, size: u32, indent: i64) -> String {
    format!(
        "<a:lvl{n}pPr marL=\"{}\" indent=\"-228600\" algn=\"l\" defTabSz=\"914400\" rtl=\"0\" eaLnBrk=\"1\" latinLnBrk=\"0\" hangingPunct=\"1\">\
         <a:lnSpc><a:spcPct val=\"90000\"/></a:lnSpc><a:spcBef><a:spcPts val=\"1000\"/></a:spcBef>\
         <a:buFont typeface=\"Arial\" panose=\"020B0604020202020204\"/><a:buChar char=\"&#8226;\"/>\
         <a:defRPr sz=\"{size}\" kern=\"1200\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill>\
         <a:latin typeface=\"+mn-lt\"/><a:ea typeface=\"+mn-ea\"/><a:cs typeface=\"+mn-cs\"/></a:defRPr></a:lvl{n}pPr>",
        indent
    )
}

fn plain_levels() -> String {
    (1..=9u8)
        .map(|n| {
            format!(
                "<a:lvl{n}pPr marL=\"{}\" algn=\"l\" defTabSz=\"914400\" rtl=\"0\" eaLnBrk=\"1\" latinLnBrk=\"0\" hangingPunct=\"1\">\
                 <a:defRPr sz=\"1800\" kern=\"1200\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill>\
                 <a:latin typeface=\"+mn-lt\"/><a:ea typeface=\"+mn-ea\"/><a:cs typeface=\"+mn-cs\"/></a:defRPr></a:lvl{n}pPr>",
                (n as i64 - 1) * 457200
            )
        })
        .collect()
}

fn theme() -> String {
    let clr = |name: &str, v: &str| format!("<a:{name}><a:srgbClr val=\"{v}\"/></a:{name}>");
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let line = |w: u32| {
        format!(
            "<a:ln w=\"{w}\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>\
             <a:prstDash val=\"solid\"/><a:miter lim=\"800000\"/></a:ln>"
        )
    };
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        "{DECL}<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Office Theme\"><a:themeElements>\
         <a:clrScheme name=\"Office\"><a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1>\
         <a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>{}{}{}{}{}{}{}{}{}{}</a:clrScheme>\
         <a:fontScheme name=\"Office\"><a:majorFont><a:latin typeface=\"Calibri Light\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme>\
         <a:fmtScheme name=\"Office\"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst>\
         <a:lnStyleLst>{}{}{}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst>\
         <a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements>\
         <a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
        clr("dk2", "44546A"),
        clr("lt2", "E7E6E6"),
        clr("accent1", "4472C4"),
        clr("accent2", "ED7D31"),
        clr("accent3", "A5A5A5"),
        clr("accent4", "FFC000"),
        clr("accent5", "5B9BD5"),
        clr("accent6", "70AD47"),
        clr("hlink", "0563C1"),
        clr("folHlink", "954F72"),
        line(6350),
        line(12700),
        line(19050),
    )
}

fn master() -> String {
    let title_body = "<a:bodyPr vert=\"horz\" lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\" rtlCol=\"0\" anchor=\"ctr\"><a:normAutofit/></a:bodyPr><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>Click to edit Master title style</a:t></a:r><a:endParaRPr lang=\"en-US\"/></a:p>";
    let body_body = "<a:bodyPr vert=\"horz\" lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\" rtlCol=\"0\"><a:normAutofit/></a:bodyPr><a:lstStyle/><a:p><a:pPr lvl=\"0\"/><a:r><a:rPr lang=\"en-US\"/><a:t>Click to edit Master text styles</a:t></a:r></a:p>";
    let title = ph(
        2,
        "Title Placeholder 1",
        "title",
        None,
        Some([838200, 365125, 10515600, 1325563]),
        title_body,
    );
    let body = ph(
        3,
        "Text Placeholder 2",
        "body",
        Some(1),
        Some([838200, 1825625, 10515600, 4351338]),
        body_body,
    );
    let title_style = "<a:lvl1pPr algn=\"l\" defTabSz=\"914400\" rtl=\"0\" eaLnBrk=\"1\" latinLnBrk=\"0\" hangingPunct=\"1\">\
        <a:lnSpc><a:spcPct val=\"90000\"/></a:lnSpc><a:spcBef><a:spcPct val=\"0\"/></a:spcBef><a:buNone/>\
        <a:defRPr sz=\"4400\" kern=\"1200\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill>\
        <a:latin typeface=\"+mj-lt\"/><a:ea typeface=\"+mj-ea\"/><a:cs typeface=\"+mj-cs\"/></a:defRPr></a:lvl1pPr>";
    let sizes = [2800, 2400, 2000, 1800, 1800, 1800, 1800, 1800, 1800];
    let body_style: String = (1..=9u8)
        .map(|n| level(n, sizes[n as usize - 1], 228600 + (n as i64 - 1) * 457200))
        .collect();
    format!(
        "{DECL}<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx=\"1001\"><a:schemeClr val=\"bg1\"/></p:bgRef></p:bg>\
         <p:spTree>{GRP}{title}{body}</p:spTree></p:cSld>\
         <p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
         <p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/><p:sldLayoutId id=\"2147483650\" r:id=\"rId2\"/></p:sldLayoutIdLst>\
         <p:txStyles><p:titleStyle>{title_style}</p:titleStyle><p:bodyStyle>{body_style}</p:bodyStyle>\
         <p:otherStyle><a:defPPr><a:defRPr lang=\"en-US\"/></a:defPPr>{}</p:otherStyle></p:txStyles></p:sldMaster>",
        plain_levels()
    )
}

fn layout(name: &str, ty: &str, shapes: &str) -> String {
    format!(
        "{DECL}<p:sldLayout {NS} type=\"{ty}\" preserve=\"1\"><p:cSld name=\"{name}\"><p:spTree>{GRP}{shapes}</p:spTree></p:cSld>\
         <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
    )
}

fn empty_body(anchor_bottom: bool, centered: bool, size: Option<u32>) -> String {
    let anchor = if anchor_bottom { " anchor=\"b\"" } else { "" };
    let lst = match (centered, size) {
        (true, Some(sz)) => format!(
            "<a:lstStyle><a:lvl1pPr marL=\"0\" indent=\"0\" algn=\"ctr\"><a:buNone/><a:defRPr sz=\"{sz}\"/></a:lvl1pPr></a:lstStyle>"
        ),
        (false, Some(sz)) => {
            format!("<a:lstStyle><a:lvl1pPr><a:defRPr sz=\"{sz}\"/></a:lvl1pPr></a:lstStyle>")
        }
        _ => "<a:lstStyle/>".into(),
    };
    format!("<a:bodyPr{anchor}/>{lst}<a:p><a:endParaRPr lang=\"en-US\"/></a:p>")
}

/// The bytes of a new `.pptx`.
pub(crate) fn new_package() -> Vec<u8> {
    let title_layout = layout(
        "Title Slide",
        "title",
        &format!(
            "{}{}",
            ph(
                2,
                "Title 1",
                "ctrTitle",
                None,
                Some([1524000, 1122363, 9144000, 2387600]),
                &empty_body(true, true, Some(6000))
            ),
            ph(
                3,
                "Subtitle 2",
                "subTitle",
                Some(1),
                Some([1524000, 3602038, 9144000, 1655762]),
                &empty_body(false, true, Some(2400))
            )
        ),
    );
    let content_layout = layout(
        "Title and Content",
        "obj",
        &format!(
            "{}{}",
            ph(
                2,
                "Title 1",
                "title",
                None,
                None,
                &empty_body(false, false, None)
            ),
            ph(
                3,
                "Content Placeholder 2",
                "",
                Some(1),
                None,
                &empty_body(false, false, None)
            )
        ),
    );
    let slide = format!(
        "{}{}{}{SLIDE_SUFFIX}",
        slide_prefix(),
        ph(
            2,
            "Title 1",
            "ctrTitle",
            None,
            None,
            "<a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p>"
        ),
        ph(
            3,
            "Subtitle 2",
            "subTitle",
            Some(1),
            None,
            "<a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang=\"en-US\"/></a:p>"
        )
    );
    let presentation = format!(
        "{DECL}<p:presentation {NS} saveSubsetFonts=\"1\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
         <p:sldIdLst><p:sldId id=\"256\" r:id=\"rId2\"/></p:sldIdLst>\
         <p:sldSz cx=\"12192000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/>\
         <p:defaultTextStyle><a:defPPr><a:defRPr lang=\"en-US\"/></a:defPPr>{}</p:defaultTextStyle></p:presentation>",
        plain_levels()
    );
    let content_types = format!(
        "{DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
         <Override PartName=\"/ppt/presentation.xml\" ContentType=\"{CT}.presentation.main+xml\"/>\
         <Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"{CT}.slideMaster+xml\"/>\
         <Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"{CT}.slideLayout+xml\"/>\
         <Override PartName=\"/ppt/slideLayouts/slideLayout2.xml\" ContentType=\"{CT}.slideLayout+xml\"/>\
         <Override PartName=\"/ppt/slides/slide1.xml\" ContentType=\"{CT}.slide+xml\"/>\
         <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\
         <Override PartName=\"/ppt/presProps.xml\" ContentType=\"{CT}.presProps+xml\"/>\
         <Override PartName=\"/ppt/viewProps.xml\" ContentType=\"{CT}.viewProps+xml\"/>\
         <Override PartName=\"/ppt/tableStyles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml\"/>\
         <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
         <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
         </Types>"
    );
    let parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", content_types),
        (
            "_rels/.rels",
            rels(&[
                ("rId1", "officeDocument", "ppt/presentation.xml"),
                (
                    "rId2",
                    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
                    "docProps/core.xml",
                ),
                ("rId3", "extended-properties", "docProps/app.xml"),
            ]),
        ),
        (
            "docProps/core.xml",
            format!(
                "{DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
                 xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
                 xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title></dc:title><dc:creator>Kantor</dc:creator></cp:coreProperties>"
            ),
        ),
        (
            "docProps/app.xml",
            format!(
                "{DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\">\
                 <Application>Kantor</Application><PresentationFormat>Widescreen</PresentationFormat></Properties>"
            ),
        ),
        ("ppt/presentation.xml", presentation),
        (
            "ppt/_rels/presentation.xml.rels",
            rels(&[
                ("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
                ("rId2", "slide", "slides/slide1.xml"),
                ("rId3", "presProps", "presProps.xml"),
                ("rId4", "viewProps", "viewProps.xml"),
                ("rId5", "theme", "theme/theme1.xml"),
                ("rId6", "tableStyles", "tableStyles.xml"),
            ]),
        ),
        (
            "ppt/presProps.xml",
            format!("{DECL}<p:presentationPr {NS}/>"),
        ),
        (
            "ppt/viewProps.xml",
            format!("{DECL}<p:viewPr {NS}><p:gridSpacing cx=\"76200\" cy=\"76200\"/></p:viewPr>"),
        ),
        (
            "ppt/tableStyles.xml",
            format!(
                "{DECL}<a:tblStyleLst xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" def=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\"/>"
            ),
        ),
        ("ppt/theme/theme1.xml", theme()),
        ("ppt/slideMasters/slideMaster1.xml", master()),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            rels(&[
                ("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
                ("rId2", "slideLayout", "../slideLayouts/slideLayout2.xml"),
                ("rId3", "theme", "../theme/theme1.xml"),
            ]),
        ),
        ("ppt/slideLayouts/slideLayout1.xml", title_layout),
        (
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]),
        ),
        ("ppt/slideLayouts/slideLayout2.xml", content_layout),
        (
            "ppt/slideLayouts/_rels/slideLayout2.xml.rels",
            rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]),
        ),
        ("ppt/slides/slide1.xml", slide),
        (
            "ppt/slides/_rels/slide1.xml.rels",
            rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]),
        ),
    ];
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in parts {
        // Writing into memory can't fail short of allocation failure.
        zip.start_file(name, opts).expect("zip entry");
        zip.write_all(content.as_bytes()).expect("zip write");
    }
    zip.finish().expect("zip finish").into_inner()
}
