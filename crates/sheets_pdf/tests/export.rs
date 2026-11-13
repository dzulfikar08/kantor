//! PDF export of `tests/corpus/print/print.xlsx` (written by
//! `scripts/corpus/gen_print_xlsx.py`; its docstring describes the sheets).
//! Page counts are worked out by hand from the file's row heights, column
//! widths, paper and margins, so they don't depend on installed fonts.

use sheets_engine::Workbook;
use sheets_pdf::{Options, Plan, Shaper};

fn fixture() -> Workbook {
    Workbook::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/corpus/print/print.xlsx"
    ))
    .unwrap()
}

fn sheet(book: &Workbook, name: &str) -> u32 {
    book.sheet_names().iter().position(|n| n == name).unwrap() as u32
}

fn plan(book: &Workbook, opts: &Options) -> Plan {
    Plan::new(book, opts, &mut Shaper::new())
}

fn only(book: &Workbook, name: &str) -> Options {
    Options {
        sheets: Some(vec![sheet(book, name)]),
        date: "11/11/2026".into(),
        time: "9:00 AM".into(),
        file_name: "print.xlsx".into(),
        ..Options::default()
    }
}

/// Text of each page, extracted by lopdf.
fn page_texts(pdf: &[u8]) -> Vec<String> {
    let doc = lopdf::Document::load_mem(pdf).unwrap();
    let pages = doc.get_pages();
    pages
        .keys()
        .map(|n| doc.extract_text(&[*n]).unwrap_or_default())
        .collect()
}

/// `(width, height)` of each page's MediaBox, points.
fn page_sizes(pdf: &[u8]) -> Vec<(f32, f32)> {
    let doc = lopdf::Document::load_mem(pdf).unwrap();
    doc.get_pages()
        .values()
        .map(|id| {
            let page = doc.get_dictionary(*id).unwrap();
            let mb = page.get(b"MediaBox").unwrap().as_array().unwrap();
            let n = |i: usize| mb[i].as_float().unwrap();
            (n(2) - n(0), n(3) - n(1))
        })
        .collect()
}

#[test]
fn report_breaks_titles_and_margins() {
    let book = fixture();
    let p = plan(&book, &only(&book, "Report"));
    let s = &p.sheets[0];
    // A4 portrait, 1 in top and bottom margins: 697.9 pt for rows of
    // 20 pt is 34 rows; later pages lose a row to the repeated title row;
    // a manual break starts row 51 (0-based 50) on a new page.
    assert!((s.page.0 - 595.28).abs() < 0.1 && (s.page.1 - 841.89).abs() < 0.1);
    let (_, grid) = &s.areas[0];
    assert_eq!(grid.rows, vec![(0, 33), (34, 49), (50, 82), (83, 100)]);
    assert_eq!(grid.cols, vec![(0, 5)]);
    assert_eq!(grid.scale, 1.0);
    assert_eq!(s.print.title_rows, Some((0, 0)));
    assert_eq!(p.pages.len(), 4);
}

#[test]
fn fit_to_width_and_order() {
    let book = fixture();
    let p = plan(&book, &only(&book, "Wide"));
    let s = &p.sheets[0];
    // 30 columns of width 10 (stored as 10.71 with padding: 75 px =
    // 56.25 pt each, 1687.5 pt) onto Letter landscape less 0.7 in margins
    // (691.2 pt): 40 %.
    assert_eq!(s.col_width(0), 56.25);
    assert_eq!((s.page.0, s.page.1), (792.0, 612.0));
    let (_, grid) = &s.areas[0];
    assert_eq!(grid.scale, 0.40);
    assert_eq!((grid.cols.len(), grid.rows.len()), (1, 1));
    assert!(s.print.setup.over_then_down);
    assert!(s.heading.0 > 0.0, "row and column headings");
}

#[test]
fn print_area_titles_and_first_page_number() {
    let book = fixture();
    let p = plan(&book, &only(&book, "Area"));
    let s = &p.sheets[0];
    // F4 / Folio: 8.5 × 13 in.
    assert_eq!((s.page.0.round(), s.page.1.round()), (612.0, 936.0));
    assert_eq!(s.areas.len(), 1);
    assert_eq!(
        s.areas[0].0,
        sheets_engine::CellRange::from_corners((2, 1), (39, 4))
    );
    assert_eq!(s.print.title_cols, Some((0, 0)));
    assert_eq!(s.print.title_rows, Some((0, 1)));
    assert_eq!(p.pages[0].number, 7);
    let (pdf, pages) = sheets_pdf::export(&mut fixture(), &only(&book, "Area")).unwrap();
    assert_eq!(pages, 1);
    let text = &page_texts(&pdf)[0];
    assert!(
        text.contains("Title row") && text.contains("Col 1"),
        "{text}"
    );
    assert!(text.contains("R40") && !text.contains("R41"), "{text}");
    assert!(!text.contains("OUTSIDEAREA"), "{text}");
}

#[test]
fn whole_workbook_pdf() {
    let mut book = fixture();
    let opts = Options {
        date: "11/11/2026".into(),
        ..Options::default()
    };
    let p = plan(&book, &opts);
    // The hidden sheet is left out.
    let names: Vec<&str> = p.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Report", "Wide", "Styled", "Area", "Legal"]);
    let (pdf, pages) = sheets_pdf::export(&mut book, &opts).unwrap();
    assert_eq!(pages, p.pages.len());
    let texts = page_texts(&pdf);
    let sizes = page_sizes(&pdf);
    assert_eq!(texts.len(), pages);
    // Report: header and footer fields, the title row on every page.
    assert!(texts[0].contains("Quarterly Report") && texts[0].contains("11/11/2026"));
    assert!(
        texts[0].contains(&format!("Page 1 of {pages}")),
        "{}",
        texts[0]
    );
    assert!(
        texts[2].contains("Order") && texts[2].contains("1049"),
        "{}",
        texts[2]
    );
    assert!(!texts[1].contains("1049"));
    // Legal paper last, 8.5 × 14 in; its used range only.
    assert_eq!(
        sizes.last().map(|s| (s.0.round(), s.1.round())),
        Some((612.0, 1008.0))
    );
    assert!(texts.last().unwrap().contains("Legal paper"));
    let all = texts.concat();
    assert!(all.contains("Merged and centred"));
    assert!(all.contains("####"), "a number too wide for its column");
    assert!(all.contains("after hidden"));
    for absent in ["SECRETSHEET", "HIDDENROW", "HIDDENCOL"] {
        assert!(!all.contains(absent), "{absent} printed");
    }
}

#[test]
fn overrides() {
    let book = fixture();
    let opts = Options {
        paper: Some(sheets_engine::Paper::A4),
        landscape: Some(true),
        fit: Some((1, 1)),
        ..only(&book, "Report")
    };
    let p = plan(&book, &opts);
    assert_eq!(p.pages.len(), 1);
    assert!(p.sheets[0].page.0 > p.sheets[0].page.1);
    // Overrides on a sheet without page setup: the used range only.
    let p = plan(&book, &only(&book, "Legal"));
    assert_eq!(
        p.sheets[0].areas[0].0,
        sheets_engine::CellRange::from_corners((4, 2), (5, 3))
    );
}

#[test]
fn unapplied_settings_preview_without_touching_the_workbook() {
    let book = fixture();
    let s = sheet(&book, "Report");
    let before = plan(&book, &only(&book, "Report")).pages.len();
    let mut print = book.print_settings(s);
    print.setup.landscape = !print.setup.landscape;
    print.setup.fit = Some((1, 1));
    let opts = Options {
        print: Some((s, print)),
        ..only(&book, "Report")
    };
    let p = plan(&book, &opts);
    assert_eq!(p.pages.len(), 1, "fit to one page");
    assert!(before > 1);
    assert_eq!(plan(&book, &only(&book, "Report")).pages.len(), before);
}

#[test]
fn preview_pages_are_the_pdf_pages_rasterized() {
    let mut book = fixture();
    let opts = only(&book, "Report");
    let p = plan(&book, &opts);
    let page = sheets_pdf::extract(&mut book, &p, 0);
    let mut r = sheets_pdf::preview::Rasterizer::new();
    // The one-page PDF is the export's first page: same size, same text.
    let one = r.one_page_pdf(&page).unwrap();
    let (full, _) = sheets_pdf::export(&mut book, &opts).unwrap();
    assert_eq!(page_sizes(&one)[0], page_sizes(&full)[0]);
    assert_eq!(page_texts(&one)[0], page_texts(&full)[0]);
    // Rasterized at 1 px per point: the page's size, ink on white.
    let img = r.render(&page, 1.0).unwrap();
    let (w, h) = page_sizes(&one)[0];
    assert!((img.width as f32 - w).abs() <= 1.0 && (img.height as f32 - h).abs() <= 1.0);
    assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
    let ink = img
        .rgba
        .chunks(4)
        .filter(|px| px[0] < 128 && px[1] < 128 && px[2] < 128)
        .count();
    assert!(ink > 50, "text and lines are drawn ({ink} dark pixels)");
    assert!(img.rgba.chunks(4).all(|px| px[3] == 255), "opaque");
    // Twice the zoom: twice the pixels each way.
    let big = r.render(&page, 2.0).unwrap();
    assert!((big.width as i64 - 2 * img.width as i64).abs() <= 1);
}
