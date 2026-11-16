//! IMAGE and WEBSERVICE downloading (`sheets_engine::web`) against a local
//! HTTP server on 127.0.0.1 (no other network), the session cache, and the
//! pictures Excel saves for IMAGE cells (rich values): read when a workbook
//! opens, written back on save.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sheets_engine::Workbook;
use sheets_engine::web::{self, WebKind, WebResult};

/// A `width` × `height` PNG (header and IHDR: all the size reader needs).
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&width.to_be_bytes());
    v.extend_from_slice(&height.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    v
}

/// A server on 127.0.0.1 answering `routes` (path -> status and body), with
/// a count of the requests it got per path. Runs until the test ends.
struct Server {
    base: String,
    hits: Arc<std::sync::Mutex<BTreeMap<String, usize>>>,
    total: Arc<AtomicUsize>,
}

impl Server {
    fn start(routes: Vec<(&'static str, u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits: Arc<std::sync::Mutex<BTreeMap<String, usize>>> = Arc::default();
        let total = Arc::new(AtomicUsize::new(0));
        let (h, t) = (hits.clone(), total.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                *h.lock().unwrap().entry(path.clone()).or_default() += 1;
                t.fetch_add(1, Ordering::SeqCst);
                let (status, body) = routes
                    .iter()
                    .find(|r| r.0 == path)
                    .map_or((404, b"missing".to_vec()), |r| (r.1, r.2.clone()));
                let extra = if status == 302 {
                    "Location: /elsewhere\r\n"
                } else {
                    ""
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Server { base, hits, total }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

/// Fetch what the workbook waits for (through the session cache) and
/// recalculate, as the app does, until nothing is left.
fn download_all(wb: &mut Workbook) {
    for _ in 0..4 {
        let requests = wb.web_requests();
        if requests.is_empty() {
            return;
        }
        for (url, kind) in requests {
            let result = web::fetch_cached(&url, kind);
            wb.set_web_result(&url, kind, result);
        }
        wb.recalculate();
    }
}

#[test]
fn webservice_downloads_text_and_errors_follow_excel() {
    let server = Server::start(vec![
        ("/rate", 200, b"1.0842".to_vec()),
        ("/broken", 500, b"oops".to_vec()),
        ("/moved", 302, Vec::new()),
        ("/elsewhere", 200, b"moved here".to_vec()),
    ]);
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_web_enabled(true);
    // WEBSERVICE takes http (Excel does); a failure is #VALUE!.
    let rate = server.url("/rate");
    wb.set_input(0, 0, 0, &format!("=WEBSERVICE(\"{rate}\")"))
        .unwrap();
    wb.set_input(
        0,
        1,
        0,
        &format!("=WEBSERVICE(\"{}\")", server.url("/broken")),
    )
    .unwrap();
    wb.set_input(
        0,
        2,
        0,
        &format!("=WEBSERVICE(\"{}\")", server.url("/moved")),
    )
    .unwrap();
    wb.set_input(0, 3, 0, "=WEBSERVICE(\"http://127.0.0.1:1/closed\")")
        .unwrap();
    wb.set_input(0, 4, 0, "=A1*2").unwrap();
    assert_eq!(wb.display_value(0, 0, 0), "#BUSY!");
    download_all(&mut wb);
    assert_eq!(wb.display_value(0, 0, 0), "1.0842");
    assert_eq!(wb.display_value(0, 1, 0), "#VALUE!");
    assert_eq!(wb.display_value(0, 2, 0), "moved here");
    assert_eq!(wb.display_value(0, 3, 0), "#VALUE!");
    // Dependents are recalculated with the answer.
    assert_eq!(wb.display_value(0, 4, 0), "2.1684");
    assert_eq!(server.hits("/rate"), 1);

    // Edits and automatic recalculations use what was fetched.
    wb.set_input(0, 5, 0, "x").unwrap();
    wb.recalculate();
    download_all(&mut wb);
    assert_eq!(server.hits("/rate"), 1);
    // F9 fetches WEBSERVICE again (and failures), as Excel's full
    // recalculation does.
    wb.refresh_web();
    web::forget_text();
    wb.recalculate();
    assert_eq!(wb.display_value(0, 0, 0), "#BUSY!");
    download_all(&mut wb);
    assert_eq!(server.hits("/rate"), 2);
    assert_eq!(wb.display_value(0, 0, 0), "1.0842");
}

#[test]
fn image_downloads_once_per_session_over_https_only() {
    let server = Server::start(vec![
        ("/logo.png", 200, png(64, 32)),
        ("/page", 200, b"<html>".to_vec()),
        ("/moved.png", 302, Vec::new()),
    ]);
    // IMAGE takes https only (Excel's rule): the test server's plain http
    // URL is #VALUE!, so the fetcher is checked directly for pictures.
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_web_enabled(true);
    wb.set_input(0, 0, 0, &format!("=IMAGE(\"{}\")", server.url("/logo.png")))
        .unwrap();
    assert_eq!(wb.display_value(0, 0, 0), "#VALUE!");
    assert!(wb.web_requests().is_empty());
    assert_eq!(server.total.load(Ordering::SeqCst), 0);

    let logo = server.url("/logo.png");
    match web::fetch_cached(&logo, WebKind::Image) {
        WebResult::Image { width, height, .. } => assert_eq!((width, height), (64, 32)),
        other => panic!("{other:?}"),
    }
    // Cached for the session: another workbook gets it without a download.
    assert!(matches!(
        web::fetch_cached(&logo, WebKind::Image),
        WebResult::Image { .. }
    ));
    assert_eq!(server.hits("/logo.png"), 1);
    assert!(web::cached(&logo, WebKind::Image).is_some());
    // F9 keeps pictures.
    web::forget_text();
    assert!(web::cached(&logo, WebKind::Image).is_some());
    // Not a picture: #VALUE!; a redirect is blocked, as Excel blocks it.
    assert!(matches!(
        web::fetch_cached(&server.url("/page"), WebKind::Image),
        WebResult::Error(e) if e.to_string() == "#VALUE!"
    ));
    assert!(matches!(
        web::fetch(&server.url("/moved.png"), WebKind::Image),
        WebResult::Error(e) if e.to_string() == "#BLOCKED!"
    ));
    // Failures aren't cached (the next F9 tries again).
    assert!(web::cached(&server.url("/page"), WebKind::Image).is_none());

    // A workbook waiting for a URL the session has: answered at once.
    let mut other = Workbook::new_empty().unwrap();
    other.set_web_enabled(true);
    other
        .set_input(0, 0, 0, "=IMAGE(\"https://kantor-test.invalid/a.png\")")
        .unwrap();
    assert!(!other.answer_from_cache());
    other.set_web_result(
        "https://kantor-test.invalid/a.png",
        WebKind::Image,
        web::answer(png(2, 2), WebKind::Image),
    );
    other.recalculate();
    assert!(other.cell_image(0, 0, 0).is_some());
}

#[test]
fn blocked_formulas_download_nothing() {
    let server = Server::start(vec![("/rate", 200, b"1".to_vec())]);
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(
        0,
        0,
        0,
        &format!("=WEBSERVICE(\"{}\")", server.url("/rate")),
    )
    .unwrap();
    wb.set_input(0, 1, 0, "=IMAGE(\"https://kantor-test.invalid/a.png\")")
        .unwrap();
    wb.recalculate();
    assert!(wb.has_web_formulas());
    assert_eq!(wb.display_value(0, 0, 0), "#BLOCKED!");
    assert_eq!(wb.display_value(0, 1, 0), "#BLOCKED!");
    assert!(wb.web_requests().is_empty());
    download_all(&mut wb);
    assert_eq!(server.total.load(Ordering::SeqCst), 0);

    let mut plain = Workbook::new_empty().unwrap();
    plain.set_input(0, 0, 0, "=SUM(1,2)").unwrap();
    assert!(!plain.has_web_formulas());
}

// ------------------------------------------------------------- rich values

const RICH_DATA: &str = "http://schemas.microsoft.com/office/spreadsheetml/2017/richdata";
const WEB_IMAGE: &str = "http://schemas.microsoft.com/office/spreadsheetml/2020/richdatawebimage";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Rewrites parts of an xlsx: `edit(name, text)` returns the new text (or
/// `None` to keep it); `add` are new parts.
fn rewrite(path: &Path, edit: impl Fn(&str, &str) -> Option<String>, add: &[(&str, Vec<u8>)]) {
    let bytes = std::fs::read(path).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).unwrap();
        let name = file.name().unwrap().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        if let Ok(text) = std::str::from_utf8(&data)
            && let Some(new) = edit(&name, text)
        {
            data = new.into_bytes();
        }
        out.start_file(name, options).unwrap();
        out.write_all(&data).unwrap();
    }
    for (name, data) in add {
        out.start_file(*name, options).unwrap();
        out.write_all(data).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut data = Vec::new();
        e.read_to_end(&mut data).unwrap();
        out.insert(e.name().unwrap().to_string(), data);
    }
    out
}

fn text(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(parts[name].clone()).unwrap()
}

/// A workbook as Excel saves IMAGE cells: B2 and B4 show pictures kept as
/// rich values (`_webimage`, with alt text on B2's), B3 one placed in the
/// cell (`_localImage`), C1 a dynamic array (IronCalc's metadata too).
fn rich_workbook(path: &Path) {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(
        0,
        1,
        1,
        "=IMAGE(\"https://example.com/a.png\",\"Logo\",3,30,60)",
    )
    .unwrap();
    wb.set_input(0, 3, 1, "=IMAGE(\"https://example.com/b.png\")")
        .unwrap();
    // Becomes a picture placed in the cell (an error value, no formula).
    wb.set_input(0, 2, 1, "placed").unwrap();
    wb.set_input(0, 0, 2, "=SEQUENCE(2)").unwrap();
    wb.save_as(path).unwrap();
    let sheet_cell = |xml: &str, r: &str, vm: u32| -> String {
        let start = xml.find(&format!("<c r=\"{r}\"")).unwrap();
        let end = xml[start..].find("</c>").unwrap() + start + 4;
        let cell = &xml[start..end];
        let f = cell
            .find("<f>")
            .map(|i| &cell[i..cell.find("</f>").unwrap() + 4])
            .unwrap_or("");
        format!(
            "{}<c r=\"{r}\" t=\"e\" vm=\"{vm}\">{f}<v>#VALUE!</v></c>{}",
            &xml[..start],
            &xml[end..]
        )
    };
    let metadata = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray" xmlns:xlrd="{RICH_DATA}"><metadataTypes count="2"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/><metadataType name="XLRICHVALUE" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><futureMetadata name="XLRICHVALUE" count="3"><bk><extLst><ext uri="{{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}}"><xlrd:rvb i="0"/></ext></extLst></bk><bk><extLst><ext uri="{{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}}"><xlrd:rvb i="1"/></ext></extLst></bk><bk><extLst><ext uri="{{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}}"><xlrd:rvb i="2"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata><valueMetadata count="3"><bk><rc t="2" v="0"/></bk><bk><rc t="2" v="1"/></bk><bk><rc t="2" v="2"/></bk></valueMetadata></metadata>"#
    );
    let structures = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvStructures xmlns="{RICH_DATA}" count="2"><s t="_webimage"><k n="WebImageIdentifier" t="i"/><k n="CalcOrigin" t="i"/><k n="ComputedImage" t="b"/><k n="ImageSizing" t="i"/><k n="Text" t="s"/></s><s t="_localImage"><k n="_rvRel:LocalImageIdentifier" t="i"/><k n="CalcOrigin" t="i"/></s></rvStructures>"#
    );
    let values = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvData xmlns="{RICH_DATA}" count="3"><rv s="0"><v>0</v><v>1</v><v>1</v><v>3</v><v>Logo</v></rv><rv s="1"><v>0</v><v>5</v></rv><rv s="0"><v>1</v><v>1</v><v>1</v><v>0</v><v></v></rv></rvData>"#
    );
    let web_images = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<webImagesSrd xmlns="{WEB_IMAGE}" xmlns:r="{R}"><webImageSrd><address r:id="rId1"/><blip r:id="rId2"/></webImageSrd><webImageSrd><address r:id="rId3"/><blip r:id="rId4"/></webImageSrd></webImagesSrd>"#
    );
    let web_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/a.png" TargetMode="External"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/b.png" TargetMode="External"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image2.png"/></Relationships>"#;
    let value_rel = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<richValueRels xmlns="http://schemas.microsoft.com/office/spreadsheetml/2022/richvaluerel" xmlns:r="{R}"><rel r:id="rId1"/></richValueRels>"#
    );
    let value_rel_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image3.png"/></Relationships>"#;
    rewrite(
        path,
        |name, xml| {
            match name {
            "xl/worksheets/sheet1.xml" => {
                let xml = sheet_cell(xml, "B2", 1);
                let xml = sheet_cell(&xml, "B3", 2);
                Some(sheet_cell(&xml, "B4", 3))
            }
            "xl/metadata.xml" => Some(metadata.clone()),
            "xl/_rels/workbook.xml.rels" => Some(xml.replace(
                "</Relationships>",
                "<Relationship Id=\"rId90\" Type=\"http://schemas.microsoft.com/office/2017/06/relationships/rdRichValue\" Target=\"richData/rdrichvalue.xml\"/><Relationship Id=\"rId91\" Type=\"http://schemas.microsoft.com/office/2017/06/relationships/rdRichValueStructure\" Target=\"richData/rdrichvaluestructure.xml\"/><Relationship Id=\"rId92\" Type=\"http://schemas.microsoft.com/office/2020/07/relationships/rdRichValueWebImage\" Target=\"richData/rdRichValueWebImage.xml\"/><Relationship Id=\"rId93\" Type=\"http://schemas.microsoft.com/office/2022/10/relationships/richValueRel\" Target=\"richData/richValueRel.xml\"/></Relationships>",
            )),
            "[Content_Types].xml" => Some(xml.replace(
                "</Types>",
                "<Default Extension=\"png\" ContentType=\"image/png\"/><Override PartName=\"/xl/richData/rdrichvalue.xml\" ContentType=\"application/vnd.ms-excel.rdrichvalue+xml\"/><Override PartName=\"/xl/richData/rdrichvaluestructure.xml\" ContentType=\"application/vnd.ms-excel.rdrichvaluestructure+xml\"/><Override PartName=\"/xl/richData/rdRichValueWebImage.xml\" ContentType=\"application/vnd.ms-excel.rdrichvaluewebimage+xml\"/><Override PartName=\"/xl/richData/richValueRel.xml\" ContentType=\"application/vnd.ms-excel.richvaluerel+xml\"/></Types>",
            )),
            _ => None,
        }
        },
        &[
            ("xl/richData/rdrichvalue.xml", values.into_bytes()),
            (
                "xl/richData/rdrichvaluestructure.xml",
                structures.into_bytes(),
            ),
            (
                "xl/richData/rdRichValueWebImage.xml",
                web_images.into_bytes(),
            ),
            (
                "xl/richData/_rels/rdRichValueWebImage.xml.rels",
                web_rels.as_bytes().to_vec(),
            ),
            ("xl/richData/richValueRel.xml", value_rel.into_bytes()),
            (
                "xl/richData/_rels/richValueRel.xml.rels",
                value_rel_rels.as_bytes().to_vec(),
            ),
            ("xl/media/image1.png", png(40, 20)),
            ("xl/media/image2.png", png(10, 10)),
            ("xl/media/image3.png", png(8, 8)),
        ],
    );
}

#[test]
fn pictures_excel_saved_show_offline_and_are_kept_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("rich.xlsx");
    rich_workbook(&original);

    let mut wb = Workbook::open(&original).unwrap();
    assert!(wb.has_saved_pictures());
    assert!(wb.has_web_formulas());
    // Before any recalculation: the saved pictures, sized by the formula.
    let b2 = wb.cell_image(0, 1, 1).unwrap();
    assert_eq!((b2.pixel_width, b2.pixel_height), (40, 20));
    assert_eq!(
        (b2.sizing, b2.height, b2.width),
        (3, Some(30.0), Some(60.0))
    );
    assert_eq!(b2.alt_text, "Logo");
    assert_eq!(wb.cell_image(0, 2, 1).unwrap().pixel_width, 8);
    assert_eq!(wb.cell_image(0, 3, 1).unwrap().pixel_width, 10);
    // Blocked (fetching off): recalculating keeps them.
    wb.recalculate();
    assert_eq!(wb.display_value(0, 1, 1), "Logo");
    assert_eq!(wb.cell_image(0, 1, 1).unwrap().pixel_width, 40);
    assert_eq!(wb.cell_image(0, 3, 1).unwrap().pixel_width, 10);
    assert_eq!(wb.cell_image(0, 2, 1).unwrap().pixel_width, 8);
    assert!(wb.web_requests().is_empty());

    // Saved: the cells, metadata and rich data as Excel writes them.
    let saved = dir.path().join("saved.xlsx");
    wb.save_as(&saved).unwrap();
    assert_eq!(wb.save_notes(), &[] as &[String], "{:?}", wb.save_notes());
    let p = parts(&saved);
    let sheet = text(&p, "xl/worksheets/sheet1.xml");
    assert!(
        sheet.contains(r#"<c r="B2" t="e" vm="1"><f>_xlfn.IMAGE("#),
        "{sheet}"
    );
    assert!(sheet.contains(r#"<v>#VALUE!</v></c>"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="B4" t="e" vm="3">"#), "{sheet}");
    // B3's picture was placed in the cell (no formula): kept too.
    assert!(
        sheet.contains(r#"<c r="B3" t="e" vm="2"><v>#VALUE!</v></c>"#),
        "{sheet}"
    );
    let meta = text(&p, "xl/metadata.xml");
    assert!(
        meta.contains("XLDAPR") && meta.contains("XLRICHVALUE"),
        "{meta}"
    );
    assert!(meta.contains("<valueMetadata count=\"3\">"), "{meta}");
    for part in [
        "xl/richData/rdrichvalue.xml",
        "xl/richData/rdrichvaluestructure.xml",
        "xl/richData/rdRichValueWebImage.xml",
        "xl/media/image1.png",
    ] {
        assert!(p.contains_key(part), "{part} missing: {:?}", p.keys());
    }
    let types = text(&p, "[Content_Types].xml");
    assert!(types.contains("rdrichvaluewebimage+xml"), "{types}");
    assert!(sheets_engine::package_problems(&std::fs::read(&saved).unwrap()).is_empty());

    // Opened again: the same pictures.
    let back = Workbook::open(&saved).unwrap();
    assert_eq!(back.cell_image(0, 1, 1).unwrap().pixel_width, 40);
    assert_eq!(back.cell_image(0, 3, 1).unwrap().pixel_width, 10);

    // A cell given another formula loses its saved picture on save.
    let mut edited = Workbook::open(&saved).unwrap();
    edited
        .set_input(0, 3, 1, "=IMAGE(\"https://example.com/other.png\")")
        .unwrap();
    assert!(edited.cell_image(0, 3, 1).is_none());
    let again = dir.path().join("again.xlsx");
    edited.save_as(&again).unwrap();
    let sheet = text(&parts(&again), "xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="B2" t="e" vm="1">"#), "{sheet}");
    assert!(!sheet.contains(r#"<c r="B4" t="e" vm"#), "{sheet}");
    // Typing over a placed picture removes it.
    edited.set_input(0, 2, 1, "text").unwrap();
    assert!(edited.cell_image(0, 2, 1).is_none());

    if let Some(out) = std::env::var_os("KANTOR_RICH_VALUE_OUT") {
        let out = Path::new(&out);
        std::fs::create_dir_all(out).unwrap();
        std::fs::copy(&original, out.join("rich-original.xlsx")).unwrap();
        std::fs::copy(&saved, out.join("rich-saved.xlsx")).unwrap();
        std::fs::copy(&again, out.join("rich-edited.xlsx")).unwrap();
    }
}
