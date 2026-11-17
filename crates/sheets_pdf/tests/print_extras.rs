//! Printing what Page Setup and the file ask for beyond the cells: notes
//! (at the end of the sheet or as displayed), patterned fills, header and
//! footer pictures (`&G`) and theme colours in headers.

use std::io::{Read, Write};

use sheets_engine::{CellRange, PrintComments, StyleChange, Workbook};
use sheets_pdf::{Options, Plan, Shaper};

fn page_texts(pdf: &[u8]) -> Vec<String> {
    let doc = lopdf::Document::load_mem(pdf).unwrap();
    doc.get_pages()
        .keys()
        .map(|n| doc.extract_text(&[*n]).unwrap_or_default())
        .collect()
}

fn book_with_notes() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "Total").unwrap();
    wb.set_input(0, 1, 1, "42").unwrap();
    wb.set_note(0, 0, 0, "Checked by audit", "Ana").unwrap();
    wb.set_note(0, 1, 1, "Estimate only", "Ana").unwrap();
    wb
}

fn with_comments(wb: &mut Workbook, how: PrintComments) {
    let mut p = wb.print_settings(0);
    p.setup.comments = how;
    wb.set_print_settings(0, p).unwrap();
}

#[test]
fn notes_at_the_end_of_the_sheet() {
    let mut wb = book_with_notes();
    let (pdf, pages) = sheets_pdf::export(&mut wb, &Options::default()).unwrap();
    assert_eq!(pages, 1);
    assert!(!page_texts(&pdf)[0].contains("audit"));

    with_comments(&mut wb, PrintComments::AtEnd);
    let (pdf, pages) = sheets_pdf::export(&mut wb, &Options::default()).unwrap();
    assert_eq!(pages, 2, "the notes get a page after the cells");
    let texts = page_texts(&pdf);
    assert!(!texts[0].contains("audit"), "{}", texts[0]);
    let notes = &texts[1];
    assert!(notes.contains("Cell: A1"), "{notes}");
    assert!(notes.contains("Checked by audit"), "{notes}");
    // (lopdf may split "B2" into two words.)
    assert!(notes.contains("Estimate only"), "{notes}");
    assert!(
        notes.find("audit") < notes.find("Estimate"),
        "in cell order: {notes}"
    );
}

#[test]
fn notes_as_displayed() {
    let mut wb = book_with_notes();
    with_comments(&mut wb, PrintComments::AsDisplayed);
    // Only notes shown on the sheet print.
    let (pdf, _) = sheets_pdf::export(&mut wb, &Options::default()).unwrap();
    assert!(!page_texts(&pdf)[0].contains("audit"));
    wb.show_notes(0, CellRange::from_corners((0, 0), (0, 0)), true)
        .unwrap();
    let (pdf, pages) = sheets_pdf::export(&mut wb, &Options::default()).unwrap();
    assert_eq!(pages, 1);
    let text = &page_texts(&pdf)[0];
    assert!(text.contains("audit"), "{text}");
    assert!(!text.contains("Estimate"), "{text}");
}

/// A saved package with one part's text changed.
fn patched(
    wb: &mut Workbook,
    edits: &[(&str, &dyn Fn(String) -> String)],
    extra: &[(&str, &[u8])],
) -> Workbook {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.xlsx");
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let out_path = dir.path().join("b.xlsx");
    let mut zout = zip::ZipWriter::new(std::fs::File::create(&out_path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).unwrap();
        let name = f.name().unwrap().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if let Some((_, edit)) = edits.iter().find(|(n, _)| *n == name) {
            data = edit(String::from_utf8(data).unwrap()).into_bytes();
        }
        zout.start_file(&name, opts).unwrap();
        zout.write_all(&data).unwrap();
    }
    for (name, data) in extra {
        zout.start_file(*name, opts).unwrap();
        zout.write_all(data).unwrap();
    }
    zout.finish().unwrap();
    Workbook::open(&out_path).unwrap()
}

#[test]
fn patterned_fills_print_as_patterns() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "x").unwrap();
    wb.set_range_style(
        0,
        CellRange::from_corners((0, 0), (0, 0)),
        StyleChange::Fill(Some(0xFF0000)),
    )
    .unwrap();
    let styles = |x: String| x.replacen("patternType=\"solid\"", "patternType=\"darkDown\"", 1);
    let book = patched(&mut wb, &[("xl/styles.xml", &styles)], &[]);
    let p = book.fill_pattern(0, 0, 0).expect("a pattern");
    assert_eq!(p.kind, "darkDown");
    assert_eq!(p.fg, 0xFF0000);
    assert!(book.fill_pattern(0, 5, 5).is_none());
    let plan = Plan::new(&book, &Options::default(), &mut Shaper::new());
    let mut book = book;
    let page = sheets_pdf::extract(&mut book, &plan, 0);
    assert_eq!(page.patterns.len(), 1);
    assert_eq!(
        page.patterns[0].1,
        [0xCC, 0x66, 0x33, 0x99, 0xCC, 0x66, 0x33, 0x99]
    );
    assert!(page.fills.iter().all(|(_, c)| *c != 0xFF0000), "not solid");
}

/// A 2 × 2 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xFD, 0xD4, 0x9A,
    0x73, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x44, 0x0C, 0x60, 0xF2, 0x3F, 0x00, 0x1B, 0xF2, 0x03, 0xFD, 0xF3, 0xE7, 0x4B, 0xB1, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn header_pictures_and_theme_colours() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "x").unwrap();
    let mut p = wb.print_settings(0);
    p.header_footer.odd_header = "&L&G&C&K04+000Accent title".into();
    wb.set_print_settings(0, p).unwrap();
    let sheet = |x: String| {
        x.replacen(
            "</worksheet>",
            "<legacyDrawingHF r:id=\"rIdHF\"/></worksheet>",
            1,
        )
        .replacen(
            "<worksheet ",
            "<worksheet xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",
            usize::from(!x.contains("xmlns:r=")),
        )
    };
    let vml = br##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><v:shape id="LH" o:spid="_x0000_s1025" type="#_x0000_t75" style="position:absolute;margin-left:0;margin-top:0;width:72pt;height:36pt;z-index:1"><v:imagedata o:relid="rId1" o:title="logo"/></v:shape></xml>"##;
    let vml_rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/logo.png"/></Relationships>"#;
    let sheet_rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdHF" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawingHF1.vml"/></Relationships>"#;
    let mut book = patched(
        &mut wb,
        &[("xl/worksheets/sheet1.xml", &sheet)],
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels),
            ("xl/drawings/vmlDrawingHF1.vml", vml),
            ("xl/drawings/_rels/vmlDrawingHF1.vml.rels", vml_rels),
            ("xl/media/logo.png", PNG),
        ],
    );
    let print = book.print_settings(0);
    let pic = print
        .hf_picture(1, true, 0)
        .expect("the left header picture");
    assert_eq!((pic.width_pt, pic.height_pt), (72.0, 36.0));
    assert_eq!(&*pic.part, "xl/media/logo.png");
    assert!(print.hf_picture(1, true, 1).is_none());
    let plan = Plan::new(&book, &Options::default(), &mut Shaper::new());
    let page = sheets_pdf::extract(&mut book, &plan, 0);
    assert!(page.hf_pictures[0][0].is_some());
    assert!(page.header.left[0].iter().any(|r| r.picture));
    // Accent 1 of the default theme, not the default black.
    let accent = book.theme_rgb(4, 0.0);
    assert_ne!(accent, 0);
    assert_eq!(page.header.center[0][0].font.color, Some(accent));
    let (pdf, _) = sheets_pdf::export(&mut book, &Options::default()).unwrap();
    let doc = lopdf::Document::load_mem(&pdf).unwrap();
    let images = doc
        .objects
        .values()
        .filter(|o| {
            o.as_stream().is_ok_and(|s| {
                s.dict
                    .get(b"Subtype")
                    .is_ok_and(|t| t.as_name().ok() == Some(b"Image"))
            })
        })
        .count();
    assert!(images >= 1, "the picture is in the PDF");
}

#[test]
fn notes_as_displayed_keep_their_formatting() {
    use sheets_engine::NoteRun;
    let mut wb = book_with_notes();
    with_comments(&mut wb, PrintComments::AsDisplayed);
    let run = |text: &str, bold: bool, color: Option<u32>| NoteRun {
        text: text.into(),
        bold,
        italic: false,
        underline: color.is_some(),
        size: Some(1200),
        color,
        font: None,
        other: String::new(),
    };
    wb.set_note_runs(
        0,
        0,
        0,
        vec![run("Ana:", true, None), run("\nRed alert", false, Some(0xFF0000))],
        "Ana",
    )
    .unwrap();
    wb.show_notes(0, CellRange::from_corners((0, 0), (0, 0)), true)
        .unwrap();
    let plan = Plan::new(&wb, &Options::default(), &mut Shaper::new());
    let page = sheets_pdf::extract(&mut wb, &plan, 0);
    assert_eq!(page.notes.len(), 1);
    let rich = &page.notes[0];
    assert_eq!(rich.spans.len(), 2);
    assert!(rich.spans[0].font.bold);
    assert_eq!(rich.spans[1].color, 0xFF0000);
    assert!(rich.spans[1].underline);
    assert_eq!(rich.spans[1].font.size_pt(), 12.0);
    let (pdf, _) = sheets_pdf::export(&mut wb, &Options::default()).unwrap();
    let text = &page_texts(&pdf)[0];
    assert!(text.contains("Red") && text.contains("alert"), "{text}");
    // The red run is drawn red.
    let doc = lopdf::Document::load_mem(&pdf).unwrap();
    let (_, page_id) = doc.get_pages().into_iter().next().unwrap();
    let content = String::from_utf8_lossy(&doc.get_page_content(page_id)).into_owned();
    assert!(content.contains("1 0 0 rg"), "a red fill colour");
}
