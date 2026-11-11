//! Charts and pictures printed with their sheet, from
//! `tests/corpus/print/drawings.xlsx` (written by
//! `scripts/corpus/gen_print_drawings_xlsx.py`; its docstring describes
//! the sheets) and `tests/corpus/xlsx/20_charts.xlsx`. Page counts follow
//! from the files' 20 pt rows and page breaks, not from fonts; text checks
//! only look for short Latin words.

use std::collections::HashSet;

use lopdf::{Document, Object, ObjectId};
use sheets_engine::Workbook;
use sheets_pdf::{Options, Plan, Shaper};

fn open(rel: &str) -> Workbook {
    Workbook::open(format!(
        "{}/../../tests/corpus/{rel}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn sheet(book: &Workbook, name: &str) -> u32 {
    book.sheet_names().iter().position(|n| n == name).unwrap() as u32
}

fn only(book: &Workbook, name: &str) -> Options {
    Options {
        sheets: Some(vec![sheet(book, name)]),
        date: "11/11/2026".into(),
        time: "9:00 AM".into(),
        ..Options::default()
    }
}

fn export(book: &mut Workbook, opts: &Options) -> Document {
    let (pdf, pages) = sheets_pdf::export(book, opts).unwrap();
    let doc = Document::load_mem(&pdf).unwrap();
    assert_eq!(doc.get_pages().len(), pages);
    doc
}

fn texts(doc: &Document) -> Vec<String> {
    doc.get_pages()
        .keys()
        .map(|n| doc.extract_text(&[*n]).unwrap_or_default())
        .collect()
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap(),
        o => o,
    }
}

/// Image XObjects a resource dictionary reaches (through form XObjects):
/// `(id, filter, width, height)`.
fn images_in(
    doc: &Document,
    resources: &Object,
    seen: &mut HashSet<ObjectId>,
    out: &mut Vec<(ObjectId, String, i64, i64)>,
) {
    let Ok(res) = deref(doc, resources).as_dict() else {
        return;
    };
    let Ok(xobjects) = res.get(b"XObject") else {
        return;
    };
    let Ok(xobjects) = deref(doc, xobjects).as_dict() else {
        return;
    };
    for (_, v) in xobjects.iter() {
        let Object::Reference(id) = v else { continue };
        if !seen.insert(*id) {
            continue;
        }
        let Ok(stream) = doc.get_object(*id).and_then(|o| o.as_stream()) else {
            continue;
        };
        let d = &stream.dict;
        match d.get(b"Subtype").and_then(|s| s.as_name()) {
            Ok(b"Image") => {
                let filter = d
                    .get(b"Filter")
                    .ok()
                    .map(|f| match deref(doc, f) {
                        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
                        Object::Array(a) => a
                            .iter()
                            .filter_map(|x| x.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                            .collect::<Vec<_>>()
                            .join(","),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                let n = |k: &[u8]| d.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
                out.push((*id, filter, n(b"Width"), n(b"Height")));
            }
            Ok(b"Form") => {
                if let Ok(r) = d.get(b"Resources") {
                    images_in(doc, r, seen, out);
                }
            }
            _ => {}
        }
    }
}

/// The pictures drawn on each page (soft masks aside).
fn page_images(doc: &Document) -> Vec<Vec<(ObjectId, String, i64, i64)>> {
    doc.get_pages()
        .values()
        .map(|id| {
            let page = doc.get_dictionary(*id).unwrap();
            let mut out = Vec::new();
            if let Ok(r) = page.get(b"Resources") {
                images_in(doc, r, &mut HashSet::new(), &mut out);
            }
            // A soft mask is an image too; keep the drawn ones (with a
            // filter of their own and not referenced as /SMask).
            let masks: HashSet<ObjectId> = out
                .iter()
                .filter_map(|(id, ..)| {
                    let s = doc.get_object(*id).ok()?.as_stream().ok()?;
                    s.dict.get(b"SMask").ok()?.as_reference().ok()
                })
                .collect();
            out.retain(|(id, ..)| !masks.contains(id));
            out
        })
        .collect()
}

#[test]
fn chart_split_over_a_page_break() {
    let mut book = open("print/drawings.xlsx");
    let opts = only(&book, "Split");
    let doc = export(&mut book, &opts);
    let t = texts(&doc);
    // Pages as without drawings: the break before row 31.
    assert_eq!(t.len(), 2, "{t:?}");
    // The chart's title is in its top part (page 1), its legend in the
    // bottom part (page 2); neither is written on the other page.
    assert!(t[0].contains("Split chart"), "{}", t[0]);
    assert!(!t[1].contains("Split chart"), "{}", t[1]);
    assert!(t[1].contains("Legend"), "{}", t[1]);
    assert!(!t[0].contains("Legend"), "{}", t[0]);
}

#[test]
fn pictures_on_their_pages() {
    let mut book = open("print/drawings.xlsx");
    let opts = only(&book, "Split");
    let doc = export(&mut book, &opts);
    let imgs = page_images(&doc);
    assert_eq!(imgs.len(), 2);
    // Page 1: the logo in the title row only (the hidden picture and the
    // one set not to print are left out).
    assert_eq!(imgs[0].len(), 1, "{:?}", imgs[0]);
    assert_eq!((imgs[0][0].2, imgs[0][0].3), (16, 16), "full resolution");
    // Page 2: the photo, embedded as the JPEG it is (no re-encoding, full
    // size); the logo isn't repeated with the title row.
    assert_eq!(imgs[1].len(), 1, "{:?}", imgs[1]);
    let (_, filter, w, h) = &imgs[1][0];
    assert!(filter.contains("DCTDecode"), "{filter}");
    assert_eq!((*w, *h), (120, 80));
}

#[test]
fn hidden_rows_shrink_a_chart() {
    let book = open("print/drawings.xlsx");
    let plan = Plan::new(&book, &only(&book, "Hidden"), &mut Shaper::new());
    let sp = &plan.sheets[0];
    let d = &sp.drawings.items;
    assert_eq!(d.len(), 1);
    // The chart's rows minus the five hidden ones.
    let sheets_engine::drawing::Anchor::TwoCell { from, to, .. } =
        book.drawings(sheet(&book, "Hidden"))[0].anchor
    else {
        panic!("two-cell anchor")
    };
    let visible: f64 = (from.row..to.row).map(|r| sp.row_height(r)).sum();
    assert!((4..=8).all(|r| sp.row_height(r) == 0.0));
    let off = |emu: i64| emu as f64 / 9525.0 * 0.75;
    let want = visible - off(from.row_off) + off(to.row_off);
    assert!(
        (d[0].rect[3] - want).abs() < 0.01,
        "{} vs {want}",
        d[0].rect[3]
    );
}

#[test]
fn a_chart_beside_the_data_widens_the_printed_range() {
    let mut book = open("print/drawings.xlsx");
    let opts = only(&book, "Fit");
    let plan = Plan::new(&book, &opts, &mut Shaper::new());
    let (area, grid) = &plan.sheets[0].areas[0];
    // Data A1:D10, the chart from H2 (480 x 288 px) to column O, row 12.
    assert_eq!((area.top, area.left), (0, 0));
    assert_eq!((area.right, area.bottom), (14, 11), "{area:?}");
    assert_eq!((grid.cols.len(), grid.rows.len()), (1, 1), "fit to 1 x 1");
    let t = texts(&export(&mut book, &opts));
    assert_eq!(t.len(), 1);
    assert!(t[0].contains("Fit chart"), "{}", t[0]);
}

#[test]
fn a_print_area_clips_a_chart() {
    let mut book = open("print/drawings.xlsx");
    let opts = only(&book, "Area");
    let plan = Plan::new(&book, &opts, &mut Shaper::new());
    // The print area stays A1:D20 although the chart reaches past it.
    let (area, _) = &plan.sheets[0].areas[0];
    assert_eq!((area.right, area.bottom), (3, 19));
    let doc = export(&mut book, &opts);
    let t = texts(&doc);
    assert_eq!(t.len(), 1);
    // The title is centred over the whole chart, past column D: clipped
    // away, and not written.
    assert!(!t[0].contains("Area chart"), "{}", t[0]);
    assert!(t[0].contains("Item 19"), "{}", t[0]);
}

#[test]
fn ignoring_hidden_and_non_printing_objects() {
    let book = open("print/drawings.xlsx");
    let all = book.drawings(sheet(&book, "Split"));
    assert_eq!(all.iter().filter(|d| d.hidden).count(), 1);
    assert_eq!(all.iter().filter(|d| !d.prints).count(), 1);
    let plan = Plan::new(&book, &only(&book, "Split"), &mut Shaper::new());
    // Chart, logo and photo.
    assert_eq!(plan.sheets[0].drawings.items.len(), 3);
}

#[test]
fn corpus_charts_print_with_live_text() {
    let mut book = open("xlsx/20_charts.xlsx");
    let opts = only(&book, "Charts");
    let doc = export(&mut book, &opts);
    let all = texts(&doc).join("\n");
    for title in [
        "Sales and costs",
        "Q1 split",
        "Costs",
        "Sales vs costs",
        "Stacked",
        "Sales and margin",
    ] {
        assert!(all.contains(title), "{title} missing");
    }
    // The cropped picture, at its own resolution.
    let imgs: Vec<_> = page_images(&doc).into_iter().flatten().collect();
    assert!(imgs.iter().any(|i| (i.2, i.3) == (40, 40)), "{imgs:?}");

    // The chart on "Summary" reads the "Charts" cells now: an edit shows.
    let s = sheet(&book, "Charts");
    book.set_input(s, 1, 3, "=0.987").unwrap();
    let opts = only(&book, "Summary");
    let t = texts(&export(&mut book, &opts)).join("\n");
    assert!(t.contains("Margin by month"), "{t}");
    assert!(
        t.contains("99%") || t.contains("98.7%") || t.contains("100%"),
        "{t}"
    );
}

#[test]
fn sheets_without_drawings_print_as_before() {
    // print.xlsx has none: plans carry no drawings and the same pages.
    let book = open("print/print.xlsx");
    let plan = Plan::new(&book, &Options::default(), &mut Shaper::new());
    assert!(plan.sheets.iter().all(|s| s.drawings.is_empty()));
}
