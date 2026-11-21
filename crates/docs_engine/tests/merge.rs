//! Mail merge: a main document's merge fields filled in per record, as
//! Word's Finish & Merge does, and the data source link kept in
//! settings.xml.

use std::io::{Cursor, Read as _, Write as _};
use std::path::Path;

use docs_engine::{Document, MailMergeLink, MergeData, MergeValue, OBJECT, Query, Selection};

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const R_NS: &str =
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

/// A package with a body, a default header and (optionally) settings
/// children.
fn package(body: &str, header: &str, settings: &str, settings_rels: Option<&str>) -> Vec<u8> {
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/></Relationships>"#;
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr/></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/></w:style></w:styles>"#
    );
    let settings = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings {W_NS} {R_NS}><w:zoom w:percent="100"/>{settings}<w:defaultTabStop w:val="720"/><w:compat/></w:settings>"#
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W_NS} {R_NS}><w:body>{body}<w:sectPr><w:headerReference w:type="default" r:id="rId3"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let header = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:hdr {W_NS} {R_NS}><w:p>{header}</w:p></w:hdr>"#
    );
    let mut parts: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", types.as_bytes().to_vec()),
        ("_rels/.rels", rels.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes().to_vec()),
        ("word/document.xml", document.into_bytes()),
        ("word/styles.xml", styles.into_bytes()),
        ("word/settings.xml", settings.into_bytes()),
        ("word/header1.xml", header.into_bytes()),
    ];
    if let Some(r) = settings_rels {
        parts.push(("word/_rels/settings.xml.rels", r.as_bytes().to_vec()));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn t(s: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{s}</w:t></w:r>"#)
}

/// A complex field with a result.
fn field(instr: &str, result: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t xml:space="preserve">{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

/// `IF { MERGEFIELD f } op "v" "yes" "no"`.
fn if_field(f: &str, op_value: &str, yes: &str, no: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF </w:instrText></w:r>{}<w:r><w:instrText xml:space="preserve"> {op_value} "{yes}" "{no}" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{no}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        field(&format!("MERGEFIELD {f}"), &format!("«{f}»"))
    )
}

fn p(inner: &str) -> String {
    format!("<w:p>{inner}</w:p>")
}

fn data() -> MergeData {
    let v = MergeValue::text;
    let n = |s: &str, x: f64| MergeValue {
        text: s.into(),
        number: Some(x),
        date: None,
    };
    MergeData {
        fields: vec![
            "First Name".into(),
            "City".into(),
            "Company".into(),
            "Salary".into(),
        ],
        rows: vec![
            vec![
                v("Budi"),
                v("Jakarta"),
                v("PT Maju"),
                n("Rp 12.500.000", 12_500_000.0),
            ],
            vec![
                v("Siti Aminah"),
                v("Bandung"),
                v(""),
                n("9000000", 9_000_000.0),
            ],
            vec![v("Zoë 张"), v("Jakarta"), v("CV Sejahtera"), v("")],
        ],
    }
}

fn letter_body() -> String {
    [
        p(&format!("{}{}", t("Dear "), field("MERGEFIELD First_Name", "«First_Name»"))),
        p(&field("MERGEFIELD Company", "«Company»")),
        p(&format!(
            "{}{}",
            t("From: "),
            r#"<w:fldSimple w:instr=" MERGEFIELD City \* Upper "><w:r><w:t>«City»</w:t></w:r></w:fldSimple>"#
        )),
        p(&if_field("City", "= \"Jakarta\"", "Head office", "Branch office")),
        p(&format!(
            "{}{}",
            t("Salary: "),
            field(r##"MERGEFIELD Salary \# "#,##0" \b "Rp " "##, "«Salary»")
        )),
        p(&format!(
            "{}{}{}{}{}",
            t("Letter "),
            field("MERGESEQ", "1"),
            t(" for record "),
            field("MERGEREC", "1"),
            field("PAGE", "1")
        )),
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="4000" w:type="dxa"/></w:tcPr>"#
            .to_owned()
            + &p(&field("MERGEFIELD First_Name", "«First_Name»"))
            + "</w:tc></w:tr></w:tbl>",
        p(&format!(
            r#"<w:bookmarkStart w:id="0" w:name="End"/>{}<w:bookmarkEnd w:id="0"/>"#,
            t("Regards")
        )),
    ]
    .concat()
}

fn shown(doc: &Document) -> Vec<String> {
    doc.paragraphs()
        .iter()
        .map(|p| p.text.replace(OBJECT, ""))
        .collect()
}

fn open(dir: &Path, name: &str, bytes: &[u8]) -> Document {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    Document::open(&path).unwrap()
}

fn entry(bytes: &[u8], name: &str) -> Option<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    zip.by_name(name).ok()?.read_to_string(&mut out).unwrap();
    Some(out)
}

#[test]
fn each_record_gets_its_own_text() {
    let dir = tempfile::tempdir().unwrap();
    let header = field("MERGEFIELD Company", "«Company»");
    let doc = open(
        dir.path(),
        "letter.docx",
        &package(&letter_body(), &header, "", None),
    );
    assert_eq!(
        doc.merge_field_names(),
        vec!["First_Name", "Company", "City", "Salary"]
    );
    let data = data();
    let all: Vec<usize> = (0..3).collect();
    let copies = doc.merge_all(&data, &all);
    assert_eq!(copies.len(), 3);
    let first = shown(&copies[0].1);
    assert_eq!(
        &first[..7],
        &[
            "Dear Budi",
            "PT Maju",
            "From: JAKARTA",
            "Head office",
            "Salary: Rp 12,500,000",
            "Letter 1 for record 1",
            "Budi",
        ]
    );
    // Siti has no company: that line goes (blank line suppression); no
    // salary picture fails; Bandung is a branch.
    let second = shown(&copies[1].1);
    assert_eq!(
        &second[..6],
        &[
            "Dear Siti Aminah",
            "From: BANDUNG",
            "Branch office",
            "Salary: Rp 9,000,000",
            "Letter 2 for record 2",
            "Siti Aminah",
        ]
    );
    // Unicode names and an empty value with \b: nothing at all.
    let third = shown(&copies[2].1);
    assert_eq!(third[0], "Dear Zoë 张");
    assert_eq!(third[4], "Salary: ");
    // Merge fields became text; PAGE stays a field.
    for (_, c) in &copies {
        assert!(c.fields().is_empty());
        let pages = c
            .paragraphs()
            .iter()
            .flat_map(|p| {
                p.objects()
                    .filter(|(_, o)| docs_engine::page_field(o).is_some())
            })
            .count();
        assert_eq!(pages, 1, "PAGE stays a field");
        assert!(c.mail_merge().is_none());
    }
    // The header is merged too.
    let st = &copies[1].1.stories()[0];
    assert_eq!(shown(&st.doc)[0], "");
    let st = &copies[0].1.stories()[0];
    assert_eq!(shown(&st.doc)[0], "PT Maju");

    // Saved, each copy opens again with the same text (Word's merged
    // document: plain text where the fields were).
    for (k, (_, c)) in copies.iter().enumerate() {
        let path = dir.path().join(format!("out{k}.docx"));
        let mut c = c.clone();
        c.save_as(&path).unwrap();
        let back = Document::open(&path).unwrap();
        assert_eq!(shown(&back), shown(&c));
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !entry(&bytes, "word/document.xml")
                .unwrap()
                .contains("MERGEFIELD")
        );
        assert!(
            entry(&bytes, "word/header1.xml")
                .unwrap()
                .contains(if k == 0 {
                    "PT Maju"
                } else if k == 1 {
                    "<w:p"
                } else {
                    "CV Sejahtera"
                })
        );
    }
}

#[test]
fn next_skipif_and_nextif_move_through_records() {
    let dir = tempfile::tempdir().unwrap();
    // Two labels per page: NEXT moves on to the next record.
    let body = [
        p(&field("MERGEFIELD First_Name", "«First_Name»")),
        p(&field("NEXT", "")),
        p(&field("MERGEFIELD First_Name", "«First_Name»")),
    ]
    .concat();
    let doc = open(dir.path(), "labels.docx", &package(&body, "", "", None));
    let data = data();
    let copies = doc.merge_all(&data, &[0, 1, 2]);
    assert_eq!(copies.len(), 2);
    assert_eq!(shown(&copies[0].1), vec!["Budi", "", "Siti Aminah"]);
    // Past the last record the fields are empty (that line goes).
    assert_eq!(shown(&copies[1].1), vec!["Zoë 张", ""]);

    // SKIPIF leaves out the records it matches.
    let skip = format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> SKIPIF </w:instrText></w:r>{}<w:r><w:instrText xml:space="preserve"> = "Bandung" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>{}</w:p>"#,
        field("MERGEFIELD City", "«City»"),
        field("MERGEFIELD First_Name", "«First_Name»")
    );
    let doc = open(dir.path(), "skip.docx", &package(&skip, "", "", None));
    let copies = doc.merge_all(&data, &[0, 1, 2]);
    let names: Vec<String> = copies.iter().map(|(_, d)| shown(d)[0].clone()).collect();
    assert_eq!(names, vec!["Budi", "Zoë 张"]);
    assert_eq!(copies[1].0, 2, "the record a copy starts with");

    // NEXTIF moves on only when its condition holds.
    let nextif = format!(
        r#"<w:p>{}</w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> NEXTIF </w:instrText></w:r>{}<w:r><w:instrText xml:space="preserve"> = "Jakarta" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>{}</w:p>"#,
        field("MERGEFIELD First_Name", "«First_Name»"),
        field("MERGEFIELD City", "«City»"),
        field("MERGEFIELD First_Name", "«First_Name»")
    );
    let doc = open(dir.path(), "nextif.docx", &package(&nextif, "", "", None));
    let copies = doc.merge_all(&data, &[0, 1, 2]);
    let texts: Vec<Vec<String>> = copies.iter().map(|(_, d)| shown(d)).collect();
    assert_eq!(
        texts,
        vec![
            vec!["Budi".to_owned(), "Siti Aminah".to_owned()],
            vec!["Zoë 张".to_owned()],
        ]
    );
}

/// A field whose code holds other fields: `pre { inner } post`, with
/// `result` (the template's stale text).
fn nested(pre: &str, inner: &str, post: &str, result: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {pre} </w:instrText></w:r>{inner}<w:r><w:instrText xml:space="preserve"> {post} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

/// Formulas and COMPARE fed by merge fields, SET/ASK with REF and FILLIN
/// are worked out per record, as Word 365 merges docx4j's CR-067 probe
/// (its merged letters show 300 for `={ MERGEFIELD Amount }*2`, 151 for
/// `REF total` after `SET total { ={ MERGEFIELD Amount }+1 }`, Hello and
/// World for the ASK and FILLIN defaults, nothing for SET and ASK).
#[test]
fn formulas_set_ask_and_fillin_are_worked_out_per_record() {
    let dir = tempfile::tempdir().unwrap();
    let salary = || field("MERGEFIELD Salary", "OLD");
    let body = [
        p(&format!(
            "{}{}",
            t("Double: "),
            nested("=", &salary(), "*2 \\# \"#,##0\"", "OLDOLD")
        )),
        p(&format!(
            "{}{}",
            t("Compare: "),
            nested(
                "COMPARE",
                &field("MERGEFIELD City", "OLD"),
                "= \"Band*\"",
                "OLD"
            )
        )),
        p(&format!(
            "{}{}{}",
            t("Set: "),
            nested(
                "SET total",
                &nested("=", &salary(), "+1", "OLD"),
                "",
                "OLDOLDOLD"
            ),
            field("REF total", "OLD")
        )),
        p(&format!(
            "{}{}",
            field("ASK Greeting \"Greeting?\" \\d \"Hello\" \\o", "OLD"),
            field("REF Greeting \\* Upper", "OLD")
        )),
        p(&field("FILLIN \"Your name?\" \\d \"World\"", "OLD")),
        p(&field("= 2*3", "6")),
    ]
    .concat();
    let doc = open(dir.path(), "probe.docx", &package(&body, "", "", None));
    let data = data();
    let copies = doc.merge_all(&data, &[1, 0]);
    assert_eq!(
        shown(&copies[0].1)[..6],
        [
            "Double: 18,000,000",
            "Compare: 1",
            "Set: 9000001",
            "HELLO",
            "World",
            "6"
        ]
    );
    // Budi is in Jakarta; his salary isn't a plain number to Word's
    // formulas ("Rp 12.500.000").
    assert_eq!(shown(&copies[1].1)[1], "Compare: 0");
    // Only merge-fed fields became text: `= 2*3` is still a field.
    assert_eq!(copies[0].1.fields().len(), 1);
}

#[test]
fn one_document_with_a_section_per_record() {
    let dir = tempfile::tempdir().unwrap();
    let header = field("MERGEFIELD First_Name", "«First_Name»");
    let doc = open(
        dir.path(),
        "letter.docx",
        &package(&letter_body(), &header, "", None),
    );
    let data = data();
    let one = doc.merge_into_one(&data, &[2, 0]).unwrap();
    assert_eq!(one.sections().len(), 2);
    let text = shown(&one).join("\n");
    assert!(text.starts_with("Dear Zoë 张\nCV Sejahtera"), "{text}");
    assert!(text.contains("Dear Budi"));
    // Sorted order: MERGEREC counts the records as merged.
    assert!(text.contains("Letter 2 for record 2"));
    let path = dir.path().join("merged.docx");
    let mut one = one;
    one.save_as(&path).unwrap();
    if let Some(keep) = std::env::var_os("KANTOR_KEEP_OUT") {
        std::fs::copy(&path, Path::new(&keep).join("merged.docx")).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    let xml = entry(&bytes, "word/document.xml").unwrap();
    assert_eq!(xml.matches("<w:tbl>").count(), 2);
    assert_eq!(xml.matches("w:name=\"End\"").count(), 1, "bookmarks once");
    assert!(xml.contains("<w:type w:val=\"nextPage\"/>"));
    // Each record's section has its own header with its name.
    let back = Document::open(&path).unwrap();
    assert_eq!(back.sections().len(), 2);
    let headers: Vec<String> = back
        .stories()
        .iter()
        .map(|s| shown(&s.doc).join(""))
        .collect();
    assert!(headers.contains(&"Zoë 张".to_owned()), "{headers:?}");
    assert!(headers.contains(&"Budi".to_owned()), "{headers:?}");
    let refs: Vec<&str> = xml.matches("<w:headerReference").collect();
    assert_eq!(refs.len(), 2);
}

#[test]
fn the_data_source_link_is_kept_and_written_like_word() {
    let dir = tempfile::tempdir().unwrap();
    // As Word writes it for a CSV file.
    let settings = r#"<w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:linkToQuery/><w:dataType w:val="textFile"/><w:connectString w:val=""/><w:query w:val="SELECT * FROM C:\Users\x\people.csv"/><w:dataSource r:id="rId1"/><w:viewMergedData/><w:activeRecord w:val="2"/><w:odso><w:fHdr/></w:odso></w:mailMerge>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/mailMergeSource" Target="file:///C:\Users\x\people.csv" TargetMode="External"/></Relationships>"#;
    let bytes = package(&letter_body(), "", settings, Some(rels));
    let mut doc = open(dir.path(), "word.docx", &bytes);
    let link = doc.mail_merge().unwrap().clone();
    assert!(link.is_text_file() && link.preview);
    assert_eq!(link.active_record, Some(2));
    // Found beside the document (on Windows the original path would win if
    // it existed; it doesn't on any test machine).
    std::fs::write(dir.path().join("people.csv"), "Name\n").unwrap();
    assert_eq!(
        link.source_path(Some(dir.path())),
        Some(dir.path().join("people.csv"))
    );
    // Unchanged: settings.xml and its relationships stay byte for byte.
    let end = doc.end();
    let s = doc.insert_text(Selection::caret(end), "!", None);
    doc.backspace(s);
    doc.save().unwrap();
    let saved = std::fs::read(dir.path().join("word.docx")).unwrap();
    assert_eq!(
        entry(&saved, "word/settings.xml"),
        entry(&bytes, "word/settings.xml")
    );
    assert_eq!(
        entry(&saved, "word/_rels/settings.xml.rels"),
        entry(&bytes, "word/_rels/settings.xml.rels")
    );

    // Another data source chosen in Kantor: a relative path, Word's form.
    let q = Query::parse("SELECT * FROM `Sheet1$`");
    doc.set_mail_merge(Some(MailMergeLink::new(
        "data/staff list.xlsx",
        Some("Staff"),
        &q,
    )));
    doc.save().unwrap();
    let saved = std::fs::read(dir.path().join("word.docx")).unwrap();
    let s = entry(&saved, "word/settings.xml").unwrap();
    assert!(s.contains(r#"<w:dataType w:val="native"/>"#), "{s}");
    assert!(
        s.contains(r#"<w:query w:val="SELECT * FROM `Staff$` "/>"#),
        "{s}"
    );
    assert!(s.contains("Data Source=data\\staff list.xlsx;"), "{s}");
    assert!(!s.contains("odso"), "the old source's settings go");
    let r = entry(&saved, "word/_rels/settings.xml.rels").unwrap();
    assert!(!r.contains("people.csv"));
    assert!(
        r.contains(r#"Target="data/staff%20list.xlsx" TargetMode="External""#),
        "{r}"
    );
    // For checking with Office's validator: KANTOR_KEEP_OUT=dir.
    if let Some(keep) = std::env::var_os("KANTOR_KEEP_OUT") {
        std::fs::copy(
            dir.path().join("word.docx"),
            Path::new(&keep).join("linked.docx"),
        )
        .unwrap();
    }
    let back = Document::open(&dir.path().join("word.docx")).unwrap();
    let l = back.mail_merge().unwrap();
    assert_eq!(l.parsed_query().table, "Staff");
    assert_eq!(
        l.source_path(Some(dir.path())),
        Some(dir.path().join("data/staff list.xlsx"))
    );

    // A new document gets the link (and settings.xml.rels) too; removing
    // it takes both away.
    let mut fresh = Document::new();
    fresh.set_mail_merge(Some(MailMergeLink::new(
        "people.csv",
        None,
        &Query::default(),
    )));
    let path = dir.path().join("new.docx");
    fresh.save_as(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let s = entry(&saved, "word/settings.xml").unwrap();
    assert!(s.contains("<w:query w:val=\"SELECT * FROM people.csv \"/><w:dataSource r:id=\"rIdKantorMergeSource1\"/>"), "{s}");
    assert!(entry(&saved, "word/_rels/settings.xml.rels").is_some());
    let mut back = Document::open(&path).unwrap();
    back.set_mail_merge(None);
    back.save().unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(
        !entry(&saved, "word/settings.xml")
            .unwrap()
            .contains("mailMerge")
    );
    assert!(
        !entry(&saved, "word/_rels/settings.xml.rels")
            .unwrap()
            .contains("mailMergeSource")
    );
}

// ── Address Block and Greeting Line ──────────────────────────────────────

/// docx4j CR-067 phase 3's address probe (`address-probe-r1.docx`): the
/// same codes and the same four (made-up) recipients, with what Word 365
/// wrote when it merged them. Word left Postal Code unmatched in that run
/// (no postcode shows anywhere), so the saved matching here does too.
fn address_probe() -> (Vec<u8>, MergeData) {
    let lt = |s: &str| s.replace('<', "&lt;").replace('>', "&gt;");
    let full = r#"<<_TITLE0_ >><<_FIRST0_>><< _LAST0_>><< _SUFFIX0_>>\n<<_COMPANY_\n>><<_STREET1_\n>><<_STREET2_\n>><<_CITY_>><<, _STATE_>><< _POSTAL_>><<\n_COUNTRY_>>"#;
    let cases = [
        (
            "A01",
            format!(r#"ADDRESSBLOCK \f "{full}" \l 1033 \c 2 \e "Australia" \d"#),
        ),
        (
            "A02",
            format!(r#"ADDRESSBLOCK \f "{full}" \l 1033 \c 1 \e "Australia""#),
        ),
        (
            "A03",
            format!(r#"ADDRESSBLOCK \f "{full}" \l 1033 \c 0 \e "Australia""#),
        ),
        (
            "A04",
            r#"ADDRESSBLOCK \f "<<_FIRST0_>><< _LAST0_>>\n<<_STREET1_\n>><<_CITY_>>" \l 1033"#.into(),
        ),
        ("A05", "ADDRESSBLOCK".into()),
        (
            "G01",
            r#"GREETINGLINE \f "<<_BEFORE_ Dear >><<_TITLE0_ >><<_LAST0_>><<_AFTER_ ,>>" \l 1033 \e "Dear Sir or Madam,""#.into(),
        ),
        (
            "G02",
            r#"GREETINGLINE \f "<<_BEFORE_ Dear >><<_FIRST0_>><<_AFTER_ ,>>" \l 1033 \e "Dear friend,""#.into(),
        ),
        (
            "G03",
            r#"GREETINGLINE \f "<<_BEFORE_ To >><<_TITLE0_ >><<_FIRST0_>><< _LAST0_>><<_AFTER_ :>>" \l 1033 \e "To whom it may concern:""#.into(),
        ),
        ("G04", "GREETINGLINE".into()),
    ];
    let body: String = cases
        .iter()
        .map(|(k, code)| {
            p(&format!(
                "{}{}",
                t(&format!("{k} = ")),
                field(&lt(code), "x")
            ))
        })
        .collect();
    let columns = [
        "Title",
        "First Name",
        "Last Name",
        "Suffix",
        "Company",
        "Address 1",
        "Address 2",
        "City",
        "State",
        "Postal Code",
        "Country",
    ];
    let mapped = [
        ("Courtesy Title", 0),
        ("First Name", 1),
        ("Last Name", 2),
        ("Suffix", 3),
        ("Company", 4),
        ("Address 1", 5),
        ("Address 2", 6),
        ("City", 7),
        ("State", 8),
        ("Country or Region", 10),
    ];
    let maps: String = mapped
        .iter()
        .map(|(f, k)| {
            format!(
                r#"<w:fieldMapData><w:type w:val="dbColumn"/><w:name w:val="{}"/><w:mappedName w:val="{f}"/><w:column w:val="{k}"/><w:lid w:val="en-US"/></w:fieldMapData>"#,
                columns[*k]
            )
        })
        .collect();
    let settings = format!(
        r#"<w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:dataType w:val="textFile"/><w:query w:val="SELECT * FROM x.csv"/><w:odso>{maps}<w:fieldMapData><w:column w:val="0"/><w:lid w:val="en-US"/></w:fieldMapData></w:odso></w:mailMerge>"#
    );
    let rows = [
        [
            "Mr",
            "John",
            "Smith",
            "Jr",
            "Acme Pty Ltd",
            "1 Main St",
            "Suite 2",
            "Sydney",
            "NSW",
            "2000",
            "Australia",
        ],
        [
            "Ms",
            "Jane",
            "Doe",
            "",
            "",
            "2 High St",
            "",
            "Auckland",
            "",
            "1010",
            "New Zealand",
        ],
        [
            "",
            "Pat",
            "Lee",
            "",
            "Beta Inc",
            "3 Low Rd",
            "",
            "Perth",
            "WA",
            "6000",
            "Australia",
        ],
        [
            "",
            "Sam",
            "",
            "",
            "",
            "4 Side Ln",
            "",
            "Hobart",
            "TAS",
            "7000",
            "Australia",
        ],
    ];
    let data = MergeData {
        fields: columns.iter().map(|c| c.to_string()).collect(),
        rows: rows
            .iter()
            .map(|r| r.iter().map(|v| MergeValue::text(*v)).collect())
            .collect(),
    };
    (package(&body, "", &settings, None), data)
}

#[test]
fn address_block_and_greeting_line_match_word_365() {
    let dir = tempfile::tempdir().unwrap();
    let (bytes, data) = address_probe();
    let doc = open(dir.path(), "probe.docx", &bytes);
    assert!(doc.has_address_fields());
    let copies = doc.merge_all(&data, &[0, 1, 2, 3]);
    assert_eq!(copies.len(), 4);
    // Word's output, paragraph by paragraph, exactly. The probe wrote `\n`
    // in the formats, which Word reads as a plain "n"; with a country rule
    // (`\c`, A01–A03) Word starts a new paragraph after the name anyway.
    let word: [&[&str]; 4] = [
        &[
            "A01 = Mr John Smith Jr",
            "nAcme Pty Ltdn1 Main StnSuite 2nSydney, NSW",
            "A02 = Mr John Smith Jr",
            "nAcme Pty Ltdn1 Main StnSuite 2nSydney, NSWnAustralia",
            "A03 = Mr John Smith Jr",
            "nAcme Pty Ltdn1 Main StnSuite 2nSydney, NSWnAustralia",
            "A04 = John Smithn1 Main StnSydney",
            "A05 = Acme Pty Ltd",
            "John Smith",
            "1 Main St",
            "Suite 2",
            "Sydney NSW",
            "Australia",
            "G01 = Dear Mr Smith,",
            "G02 = Dear John,",
            "G03 = To whom it may concern:",
            "G04 = ",
        ],
        &[
            // \d: New Zealand isn't the \e country, so its layout.
            "A01 = Ms Jane Doe",
            "2 High St",
            "Auckland",
            "New Zealand",
            "A02 = Ms Jane Doe",
            "n2 High StnAucklandnNew Zealand",
            "A03 = Ms Jane Doe",
            "n2 High StnAucklandnNew Zealand",
            "A04 = Jane Doen2 High StnAuckland",
            "A05 = Jane Doe",
            "2 High St",
            "Auckland",
            "New Zealand",
            "G01 = Dear Ms Doe,",
            "G02 = Dear Jane,",
            "G03 = To whom it may concern:",
            "G04 = ",
        ],
        &[
            "A01 = Pat Lee",
            "nBeta Incn3 Low RdnPerth, WA",
            "A02 = Pat Lee",
            "nBeta Incn3 Low RdnPerth, WAnAustralia",
            "A03 = Pat Lee",
            "nBeta Incn3 Low RdnPerth, WAnAustralia",
            "A04 = Pat Leen3 Low RdnPerth",
            "A05 = Beta Inc",
            "Pat Lee",
            "3 Low Rd",
            "Perth WA",
            "Australia",
            "G01 = Dear Pat Lee,",
            "G02 = Dear Pat,",
            "G03 = To whom it may concern:",
            "G04 = ",
        ],
        &[
            "A01 = Sam",
            "n4 Side LnnHobart, TAS",
            "A02 = Sam",
            "n4 Side LnnHobart, TASnAustralia",
            "A03 = Sam",
            "n4 Side LnnHobart, TASnAustralia",
            "A04 = Samn4 Side LnnHobart",
            "A05 = Sam",
            "4 Side Ln",
            "Hobart TAS",
            "Australia",
            "G01 = Dear Sam,",
            "G02 = Dear Sam,",
            "G03 = To whom it may concern:",
            "G04 = ",
        ],
    ];
    for (k, (_, copy)) in copies.iter().enumerate() {
        assert_eq!(shown(copy), word[k], "record {}", k + 1);
    }
    // Preview Results: the block on one line.
    let view = doc.merge_preview(&data, &[0, 1, 2, 3], 1);
    assert!(
        view.values()
            .any(|v| v == "Ms Jane Doe, 2 High St, Auckland, New Zealand")
    );
    assert!(view.values().any(|v| v == "Dear Ms Doe,"));
}

/// Aspose's Russian `Field sample - ADDRESSBLOCK.docx` (the same code,
/// with its line break before `_AFTER_`): with no courtesy title Word
/// cached "Уважаемый(ая) Людмила Ивановна!".
#[test]
fn russian_greeting_without_title_matches_word() {
    let dir = tempfile::tempdir().unwrap();
    let body = p(&field(
        "GREETINGLINE \\f \"&lt;&lt;_BEFORE_ Уважаемый &gt;&gt;&lt;&lt;_TITLE0_&gt;&gt;&lt;&lt; _LAST0_&gt;&gt;\n&lt;&lt;_AFTER_ !&gt;&gt;\" \\l 1049 \\e \"Уважаемые господа!\"",
        "x",
    ));
    let doc = open(dir.path(), "ru.docx", &package(&body, "", "", None));
    let data = MergeData {
        fields: vec!["Title".into(), "First Name".into(), "Last Name".into()],
        rows: vec![
            vec![
                MergeValue::text(""),
                MergeValue::text("Людмила"),
                MergeValue::text("Ивановна"),
            ],
            vec![
                MergeValue::text(""),
                MergeValue::text(""),
                MergeValue::text(""),
            ],
        ],
    };
    let copies = doc.merge_all(&data, &[0, 1]);
    assert_eq!(shown(&copies[0].1), ["Уважаемый(ая) Людмила Ивановна!"]);
    assert_eq!(shown(&copies[1].1), ["Уважаемые господа!"]);
}

/// Match Fields is saved in `w:odso` and read back; without it the
/// columns are guessed from their names (Indonesian ones too).
#[test]
fn match_fields_are_saved_and_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let body = p(&field(
        r#"GREETINGLINE \f "&lt;&lt;_BEFORE_ Yth. &gt;&gt;&lt;&lt;_FIRST0_&gt;&gt;&lt;&lt;_AFTER_ ,&gt;&gt;" \l 1057 \e "Dengan hormat,""#,
        "«GreetingLine»",
    )) + &p(&field("ADDRESSBLOCK", "«AddressBlock»"));
    let doc = open(dir.path(), "surat.docx", &package(&body, "", "", None));
    let data = MergeData {
        fields: vec![
            "Nama".into(),
            "Alamat".into(),
            "Kota".into(),
            "Kode Pos".into(),
            "Sapaan".into(),
        ],
        rows: vec![vec![
            MergeValue::text("Budi Santoso"),
            MergeValue::text("Jl. Merdeka 1"),
            MergeValue::text("Bandung"),
            MergeValue::text("40115"),
            MergeValue::text("Bapak"),
        ]],
    };
    let merged = doc.merge_all(&data, &[0]);
    assert_eq!(
        shown(&merged[0].1),
        [
            "Yth. Budi Santoso,",
            "Budi Santoso",
            "Jl. Merdeka 1",
            "Bandung 40115"
        ]
    );
    // Match "First Name" to Sapaan instead and save.
    let mut doc = doc;
    let link = MailMergeLink::new("data.csv", None, &Query::default());
    let map = docs_engine::address::FieldMap {
        matched: vec![("First Name".into(), "Sapaan".into())],
        saved: true,
    };
    doc.set_mail_merge(Some(link.with_field_map(&map, &data.fields, "id-ID")));
    let path = dir.path().join("saved.docx");
    doc.save_as(&path).unwrap();
    let settings = entry(&std::fs::read(&path).unwrap(), "word/settings.xml").unwrap();
    assert!(settings.contains(r#"<w:name w:val="Sapaan"/><w:mappedName w:val="First Name"/>"#));
    let back = Document::open(&path).unwrap();
    let merged = back.merge_all(&data, &[0]);
    // Saved: only First Name is matched now.
    assert_eq!(shown(&merged[0].1), ["Yth. Bapak,", "Bapak"]);
}

/// A value on several lines (Alt+Enter in Excel, a quoted CSV field) keeps
/// its lines; drawings copied per record in the one-document merge get
/// their own ids (Open XML SDK: "should have unique value").
#[test]
fn multi_line_values_and_copied_drawings() {
    let dir = tempfile::tempdir().unwrap();
    let drawing = r#"<w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="914400" cy="914400"/><wp:docPr id="4" name="Shape 4"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"/></a:graphic></wp:inline></w:drawing></w:r>"#;
    let body = [
        p(&format!(
            "{}{}",
            t("Note: "),
            field("MERGEFIELD Note", "«Note»")
        )),
        p(drawing),
    ]
    .concat();
    let doc = open(dir.path(), "t.docx", &package(&body, "", "", None));
    let data = MergeData {
        fields: vec!["Note".into()],
        rows: vec![
            vec![MergeValue::text("See attached.\nThanks")],
            vec![MergeValue::text("One\r\nTwo")],
        ],
    };
    let copies = doc.merge_all(&data, &[0, 1]);
    let first = shown(&copies[0].1);
    assert_eq!(
        first[0],
        format!("Note: See attached.{}Thanks", docs_engine::LINE_BREAK)
    );
    let mut one = doc.merge_into_one(&data, &[0, 1]).unwrap();
    let path = dir.path().join("one.docx");
    one.save_as(&path).unwrap();
    let xml = entry(&std::fs::read(&path).unwrap(), "word/document.xml").unwrap();
    assert!(xml.contains("Two"));
    let ids: Vec<&str> = xml
        .match_indices("<wp:docPr id=\"")
        .map(|(i, m)| {
            let v = &xml[i + m.len()..];
            &v[..v.find('"').unwrap()]
        })
        .collect();
    assert_eq!(ids.len(), 2, "{xml}");
    assert_ne!(ids[0], ids[1]);
}
