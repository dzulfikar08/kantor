//! Merges written to files: Docs per record and in one document, Slides
//! and Sheets `{{Field}}` templates, from a CSV file and a workbook.

use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};

use docs_engine::{Document, OBJECT, Query};
use kantor_merge::docs::{DocsOut, pdf_writer};
use kantor_merge::placeholders::{FilesOut, merge_sheets, merge_slides};
use kantor_merge::{Overwrite, Recipients, parse_where, select};

const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

fn field(instr: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:b/></w:rPr><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:rPr><w:b/></w:rPr><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r><w:r><w:rPr><w:b/></w:rPr><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>«x»</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

/// An offer letter template.
fn template(dir: &Path) -> PathBuf {
    let body = format!(
        "<w:p><w:r><w:t xml:space=\"preserve\">Dear </w:t></w:r>{}<w:r><w:t>,</w:t></w:r></w:p><w:p><w:r><w:t xml:space=\"preserve\">Salary: </w:t></w:r>{}</w:p><w:p>{}</w:p>",
        field("MERGEFIELD Name"),
        field(r##"MERGEFIELD Salary \# "#,##0""##),
        field(r#"MERGEFIELD Joined \@ "d MMMM yyyy" \b "Joined " "#)
    );
    let parts: Vec<(&str, String)> = vec![
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>"#.into(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.into(),
        ),
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/></Relationships>"#.into(),
        ),
        (
            "word/settings.xml",
            format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings {W}><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/></w:settings>"#),
        ),
        (
            "word/document.xml",
            format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#),
        ),
    ];
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    let path = dir.join("offer.docx");
    std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
    path
}

fn people(dir: &Path) -> PathBuf {
    let path = dir.join("people.csv");
    std::fs::write(
        &path,
        "Name,Dept,Salary,Joined\nSiti Aminah,Sales,12500000,2026-01-05\nZoë 张,Sales,9000000,\nBudi,Ops,7000000,2025-12-01\n",
    )
    .unwrap();
    path
}

fn text(path: &Path) -> String {
    let doc = Document::open(path).unwrap();
    doc.paragraphs()
        .iter()
        .map(|p| p.text.replace(OBJECT, ""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn docs_files_per_record_and_one_document() {
    let dir = tempfile::tempdir().unwrap();
    let doc = Document::open(&template(dir.path())).unwrap();
    let rec = Recipients::open(&people(dir.path()), None, None).unwrap();
    let query = Query {
        filters: vec![parse_where("Dept=Sales").unwrap()],
        sort: vec![("Name".into(), false)],
        ..Query::default()
    };
    let rows = select(&rec.data, &query).unwrap();
    assert_eq!(rows, vec![1, 0], "Zoë before Siti (descending)");
    let out_dir = dir.path().join("out");
    let out = DocsOut {
        dir: Some(out_dir.clone()),
        docx: true,
        pdf: true,
        name: Some("{Name} - Offer".into()),
        single: Some(dir.path().join("all.docx")),
        overwrite: Overwrite::Refuse,
    };
    let mut engine = None;
    let report =
        kantor_merge::docs::merge(&doc, &rec.data, &rows, &out, &mut pdf_writer(&mut engine))
            .unwrap();
    assert_eq!(report.records, 2);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let mut names: Vec<String> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "Siti Aminah - Offer.docx",
            "Siti Aminah - Offer.pdf",
            "Zoë 张 - Offer.docx",
            "Zoë 张 - Offer.pdf",
        ]
    );
    assert_eq!(
        text(&out_dir.join("Siti Aminah - Offer.docx")),
        "Dear Siti Aminah,\nSalary: 12,500,000\nJoined 5 January 2026"
    );
    // An empty date: \b's text goes too, and so does the empty line.
    assert_eq!(
        text(&out_dir.join("Zoë 张 - Offer.docx")),
        "Dear Zoë 张,\nSalary: 9,000,000"
    );
    let pdf = std::fs::read(out_dir.join("Zoë 张 - Offer.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let all = text(&dir.path().join("all.docx"));
    assert!(all.starts_with("Dear Zoë 张,"), "{all}");
    assert!(all.contains("Dear Siti Aminah,"));
    // The merged text keeps the field's formatting (bold, as Word: the
    // formatting of the field's first character).
    let mut zip = zip::ZipArchive::new(Cursor::new(
        std::fs::read(out_dir.join("Siti Aminah - Offer.docx")).unwrap(),
    ))
    .unwrap();
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(
        xml.contains("<w:b/></w:rPr><w:t xml:space=\"preserve\">Siti Aminah</w:t>")
            || xml.contains("<w:b/></w:rPr><w:t>Siti Aminah</w:t>"),
        "{xml}"
    );
    assert!(!xml.contains("fldChar"));

    // Nothing is overwritten unless asked.
    let again =
        kantor_merge::docs::merge(&doc, &rec.data, &rows, &out, &mut pdf_writer(&mut engine));
    assert!(again.unwrap_err().to_string().contains("exists"));
}

#[test]
fn slides_certificates() {
    let dir = tempfile::tempdir().unwrap();
    // The corpus deck with placeholders put into its title (split across
    // two runs, as PowerPoint sometimes saves them).
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/pptx/deck.pptx");
    let bytes = std::fs::read(corpus).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let name = zip.by_index(i).unwrap().name().unwrap().into_owned();
        if name == "ppt/slides/slide1.xml" {
            let mut xml = String::new();
            zip.by_index(i).unwrap().read_to_string(&mut xml).unwrap();
            let xml = xml.replacen(
                "<a:t>FY2027 Plan</a:t>",
                "<a:t>Awarded to {{Na</a:t></a:r><a:r><a:t>me}}</a:t>",
                1,
            );
            assert!(xml.contains("{{Na"));
            out.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(xml.as_bytes()).unwrap();
        } else {
            out.raw_copy_file(zip.by_index_raw(i).unwrap()).unwrap();
        }
    }
    let template = dir.path().join("certificate.pptx");
    std::fs::write(&template, out.finish().unwrap().into_inner()).unwrap();
    assert_eq!(
        kantor_merge::placeholders::slides_names(&template).unwrap(),
        vec!["Name"]
    );
    let rec = Recipients::open(&people(dir.path()), None, None).unwrap();
    let out = FilesOut {
        dir: dir.path().join("certs"),
        native: true,
        pdf: true,
        name: Some("{Name}".into()),
        overwrite: Overwrite::Refuse,
    };
    let report = merge_slides(&template, &rec.data, &[0, 1], &out, &mut None).unwrap();
    assert_eq!(report.records, 2);
    assert_eq!(report.files.len(), 4);
    let pres = slides_engine::Presentation::open(&dir.path().join("certs/Zoë 张.pptx")).unwrap();
    let all: String = (0..pres.len())
        .flat_map(|i| {
            pres.slide(i)
                .shapes
                .iter()
                .filter_map(|s| s.text.as_ref().map(|t| t.plain_text()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join("|");
    assert!(all.contains("Awarded to Zoë 张"), "{all}");
    assert!(!all.contains("{{"));
}

#[test]
fn sheets_invoices_keep_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let template = dir.path().join("invoice.xlsx");
    let mut book = sheets_engine::Workbook::new_empty().unwrap();
    book.set_input(0, 0, 0, "Invoice for {{Name}} ({{Dept}})")
        .unwrap();
    book.set_input(0, 1, 0, "Salary").unwrap();
    book.set_input(0, 1, 1, "{{Salary}}").unwrap();
    book.set_input(0, 2, 1, "=B2*2").unwrap();
    book.set_input(0, 3, 1, "{{Missing}}").unwrap();
    book.save_as(&template).unwrap();
    let rec = Recipients::open(&people(dir.path()), None, None).unwrap();
    let out = FilesOut {
        dir: dir.path().join("inv"),
        native: true,
        pdf: true,
        name: None,
        overwrite: Overwrite::Refuse,
    };
    let report = merge_sheets(&template, &rec.data, &[2], &out).unwrap();
    assert_eq!(report.files.len(), 2);
    assert!(
        report.notes.iter().any(|n| n.contains("Missing")),
        "{:?}",
        report.notes
    );
    let merged = sheets_engine::Workbook::open(dir.path().join("inv/invoice 1.xlsx")).unwrap();
    assert_eq!(merged.display_value(0, 0, 0), "Invoice for Budi (Ops)");
    assert_eq!(merged.number_at(0, 1, 1).map(|n| n.0), Some(7_000_000.0));
    assert_eq!(merged.number_at(0, 2, 1).map(|n| n.0), Some(14_000_000.0));
    assert_eq!(merged.display_value(0, 3, 1), "{{Missing}}");
    assert!(
        std::fs::read(dir.path().join("inv/invoice 1.pdf"))
            .unwrap()
            .starts_with(b"%PDF")
    );
}
