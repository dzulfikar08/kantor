//! Writes `.xlsx`, `.docx` and `.pptx` files through Kantor's engines: every
//! corpus file opened and saved, opened, edited and saved (`.xls`, `.xlt`
//! and `.ods` imports too), and files that use
//! what Kantor itself writes (tables, charts, conditional formats, data
//! validation, pivot tables, page setup, panes, hidden rows and columns, row
//! heights; comments, tracked changes, headers and footers, sections; slide
//! tables, chart data, notes).
//!
//! `office_scenarios <repo root> <out dir>` writes the files and
//! `<out dir>/manifest.tsv`: `saved file \t original file (or empty) \t
//! scenario`, or `ERROR \t scenario \t message` when a scenario failed.
//! `tools/fidelity/office_check.py` runs it, validates every saved file with
//! the Open XML SDK and LibreOffice, and compares with the originals.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use docs_engine::{Document, HfKind, HfType, PageField, Position, Selection};
use sheets_engine::{
    CellRange, CfCompare, CfFormat, CfPeriod, CfRuleDef, CfSpec, CfTextOp, ChartType, ErrorStyle,
    FilterKind, FilterScope, HeaderFooter, Margins, Message, PageSetup, Paper, PivotDestination,
    PrintOptions, RowHeight, SheetPrint, Validation, ValidationOperator, ValidationType, Workbook,
    list_source, validation_operand,
};
use slides_engine::{ChartData, Presentation, Rgb, TableOp};

struct Run {
    out: PathBuf,
    manifest: String,
    failed: usize,
}

impl Run {
    /// Run one scenario; it returns the saved file and the original it came
    /// from. A panic or error is recorded, not fatal.
    fn scenario(&mut self, name: &str, f: impl FnOnce(&Path) -> Result<Option<PathBuf>>) {
        let out = self.out.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&out)));
        match result {
            Ok(Ok(original)) => {
                let saved = self.out.join(name);
                let original = original
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let _ = writeln!(self.manifest, "{}\t{original}\t{name}", saved.display());
            }
            Ok(Err(e)) => self.fail(name, &format!("{e:#}")),
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "panic".into());
                self.fail(name, &format!("panic: {msg}"));
            }
        }
    }

    fn fail(&mut self, name: &str, msg: &str) {
        eprintln!("FAILED {name}: {msg}");
        let msg = msg.replace(['\t', '\n'], " ");
        let _ = writeln!(self.manifest, "ERROR\t{name}\t{msg}");
        self.failed += 1;
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, out] = args.as_slice() else {
        anyhow::bail!("usage: office_scenarios <repo root> <out dir>");
    };
    let corpus = Path::new(root).join("tests/corpus");
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let mut run = Run {
        out: out.clone(),
        manifest: String::new(),
        failed: 0,
    };

    let mut xlsx = files(&corpus.join("xlsx"), "xlsx")?;
    xlsx.extend(files(&corpus.join("print"), "xlsx")?);
    for src in &xlsx {
        let stem = stem(src);
        run.scenario(&format!("rt_{stem}.xlsx"), |o| {
            let mut wb = Workbook::open(src)?;
            wb.save_as(o.join(format!("rt_{stem}.xlsx")))?;
            Ok(Some(src.clone()))
        });
        run.scenario(&format!("edit_{stem}.xlsx"), |o| {
            let mut wb = Workbook::open(src)?;
            sheet_edits(&mut wb)?;
            let path = o.join(format!("edit_{stem}.xlsx"));
            wb.save_as(&path)?;
            // Save the saved file again: Kantor-made parts carried twice.
            let mut again = Workbook::open(&path)?;
            again.set_input(0, 0, 0, "again")?;
            again.save_as(&path)?;
            Ok(Some(src.clone()))
        });
    }
    // Imports (.xls, .xlt, .ods) saved as .xlsx: their formats, notes, data
    // validation and conditional formats written from the model.
    let mut imports = Vec::new();
    for ext in ["xls", "xlt", "ods"] {
        imports.extend(files(&corpus.join("xls"), ext)?);
    }
    for src in &imports {
        let ext = src
            .extension()
            .map_or(String::new(), |e| e.to_string_lossy().into_owned());
        let name = format!("import_{}_{ext}.xlsx", stem(src));
        run.scenario(&name.clone(), |o| {
            let mut wb = Workbook::open(src)?;
            let path = o.join(&name);
            wb.save_as(&path)?;
            Ok(None)
        });
        let name = format!("import_edit_{}_{ext}.xlsx", stem(src));
        run.scenario(&name.clone(), |o| {
            let mut wb = Workbook::open(src)?;
            sheet_edits(&mut wb)?;
            let path = o.join(&name);
            wb.save_as(&path)?;
            let mut again = Workbook::open(&path)?;
            again.set_input(0, 0, 0, "again")?;
            again.save_as(&path)?;
            Ok(None)
        });
    }
    run.scenario("kantor_features.xlsx", |o| {
        let mut wb = data_book()?;
        kantor_features(&mut wb)?;
        let path = o.join("kantor_features.xlsx");
        wb.save_as(&path)?;
        Ok(None)
    });
    run.scenario("kantor_features_resaved.xlsx", |o| {
        let mut wb = Workbook::open(o.join("kantor_features.xlsx"))?;
        wb.insert_rows(0, 3, 2)?;
        wb.insert_columns(0, 1, 1)?;
        wb.save_as(o.join("kantor_features_resaved.xlsx"))?;
        Ok(None)
    });
    run.scenario("filters.xlsx", |o| {
        let mut wb = data_book()?;
        filters(&mut wb)?;
        wb.save_as(o.join("filters.xlsx"))?;
        Ok(None)
    });
    for (file, sheet) in [
        ("13_tables.xlsx", 0),
        ("15_sparklines_cf_dv.xlsx", 0),
        ("16_print_protect_outline.xlsx", 0),
        ("18_conditional_formatting.xlsx", 0),
        ("19_data_validation.xlsx", 0),
        ("20_charts.xlsx", 0),
    ] {
        let src = corpus.join("xlsx").join(file);
        let name = format!("features_{}.xlsx", stem(&src));
        run.scenario(&name.clone(), |o| {
            let mut wb = Workbook::open(&src)?;
            add_data(&mut wb, sheet, 60)?;
            kantor_features_at(&mut wb, sheet, 60)?;
            wb.save_as(o.join(&name))?;
            Ok(Some(src.clone()))
        });
    }
    let pivots = corpus.join("xlsx/21_pivot_tables.xlsx");
    run.scenario("pivots_21_pivot_tables.xlsx", |o| {
        let mut wb = Workbook::open(&pivots)?;
        pivot_edits(&mut wb)?;
        wb.save_as(o.join("pivots_21_pivot_tables.xlsx"))?;
        Ok(Some(pivots.clone()))
    });

    for src in files(&corpus.join("docx"), "docx")? {
        let stem = stem(&src);
        run.scenario(&format!("rt_{stem}.docx"), |o| {
            let mut doc = Document::open(&src)?;
            doc.save_as(&o.join(format!("rt_{stem}.docx")))?;
            Ok(Some(src.clone()))
        });
        run.scenario(&format!("edit_{stem}.docx"), |o| {
            let mut doc = Document::open(&src)?;
            doc_edits(&mut doc)?;
            let path = o.join(format!("edit_{stem}.docx"));
            doc.save_as(&path)?;
            let mut again = Document::open(&path)?;
            again.insert_text(Selection::caret(Position::new(0, 0)), "Again. ", None);
            again.save_as(&path)?;
            Ok(Some(src.clone()))
        });
    }
    let review_src = out.join("review.input.docx");
    std::fs::write(&review_src, review_docx()?)?;
    run.scenario("review.docx", |o| {
        let mut doc = Document::open(&review_src)?;
        review_edits(&mut doc)?;
        doc.save_as(&o.join("review.docx"))?;
        Ok(Some(review_src.clone()))
    });
    run.scenario("review_all.docx", |o| {
        let mut doc = Document::open(&review_src)?;
        let sel = Selection::caret(Position::new(0, 0));
        doc.resolve_all(true, sel);
        doc.save_as(&o.join("review_all.docx"))?;
        Ok(Some(review_src.clone()))
    });
    run.scenario("new_doc.docx", |o| {
        let mut doc = Document::new();
        let end = doc.end();
        doc.insert_text(Selection::caret(end), "A new document.", None);
        doc_edits(&mut doc)?;
        doc.save_as(&o.join("new_doc.docx"))?;
        Ok(None)
    });

    for src in files(&corpus.join("pptx"), "pptx")? {
        let stem = stem(&src);
        run.scenario(&format!("rt_{stem}.pptx"), |o| {
            let mut p = Presentation::open(&src)?;
            p.save_as(&o.join(format!("rt_{stem}.pptx")))?;
            Ok(Some(src.clone()))
        });
        run.scenario(&format!("edit_{stem}.pptx"), |o| {
            let mut p = Presentation::open(&src)?;
            slide_edits(&mut p)?;
            let path = o.join(format!("edit_{stem}.pptx"));
            p.save_as(&path)?;
            let mut again = Presentation::open(&path)?;
            again.set_notes_text(0, "Saved twice.")?;
            again.save_as(&path)?;
            Ok(Some(src.clone()))
        });
    }
    run.scenario("new_deck.pptx", |o| {
        let mut p = Presentation::new();
        let i = p.new_slide(Some(0))?;
        let k = p.add_text_box(i)?;
        p.insert_text((i, k), Selection::caret(Position::new(0, 0)), "Hello", None)?;
        p.set_notes_text(0, "Speaker notes\nsecond line")?;
        p.set_notes_text(i, "More notes")?;
        p.save_as(&o.join("new_deck.pptx"))?;
        Ok(None)
    });

    let mut f = std::fs::File::create(out.join("manifest.tsv"))?;
    f.write_all(run.manifest.as_bytes())?;
    println!("{} scenarios failed", run.failed);
    Ok(())
}

fn files(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| dir.display().to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    Ok(v)
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------- Sheets

/// Typing, row and column inserts and deletes on the first sheet: what
/// moves carried parts (shift.rs) and rewrites the sheet.
fn sheet_edits(wb: &mut Workbook) -> Result<()> {
    wb.set_input(0, 0, 0, "Edited in Kantor")?;
    wb.insert_rows(0, 2, 2)?;
    wb.insert_columns(0, 1, 1)?;
    wb.delete_rows(0, 3, 3)?;
    wb.set_row_heights(0, 0, 0, RowHeight::Points(24.0))?;
    Ok(())
}

const HEADERS: [&str; 4] = ["Region", "Product", "Units", "Revenue"];
const ROWS: [(&str, &str, f64, f64); 8] = [
    ("East", "Apples", 10.0, 120.0),
    ("West", "Pears", 4.0, 60.0),
    ("North", "Apples", 7.0, 84.0),
    ("East", "Pears", 3.0, 45.0),
    ("West", "Apples", 12.0, 144.0),
    ("North", "Plums", 5.0, 90.0),
    ("East", "Plums", 8.0, 144.0),
    ("West", "Plums", 2.0, 36.0),
];

fn data_book() -> Result<Workbook> {
    let mut wb = Workbook::new_empty()?;
    add_data(&mut wb, 0, 0)?;
    Ok(wb)
}

/// The sample data with its header on row `top` (0-based), columns A:D.
fn add_data(wb: &mut Workbook, sheet: u32, top: u32) -> Result<()> {
    let mut cells: Vec<(u32, u32, String)> = HEADERS
        .iter()
        .enumerate()
        .map(|(c, h)| (top, c as u32, h.to_string()))
        .collect();
    for (r, (region, product, units, revenue)) in ROWS.iter().enumerate() {
        let r = top + r as u32 + 1;
        cells.push((r, 0, region.to_string()));
        cells.push((r, 1, product.to_string()));
        cells.push((r, 2, units.to_string()));
        cells.push((r, 3, revenue.to_string()));
    }
    wb.set_inputs(sheet, &cells)?;
    Ok(())
}

fn kantor_features(wb: &mut Workbook) -> Result<()> {
    kantor_features_at(wb, 0, 0)?;
    // A pivot table on a new sheet, laid out, refreshed.
    let (s, row, col) = wb.insert_pivot("Sheet1!$A$1:$D$9", 0, PivotDestination::NewSheet)?;
    let mut l = wb.pivot_layout(s, row, col).context("pivot layout")?;
    let region = l.fields.iter().position(|f| f.name == "Region").unwrap();
    let product = l.fields.iter().position(|f| f.name == "Product").unwrap();
    let revenue = l.fields.iter().position(|f| f.name == "Revenue").unwrap();
    l.toggle_field(region);
    l.toggle_field(revenue);
    l.move_entry(sheets_engine::AreaEntry::Field(product), "Columns", 0);
    wb.set_pivot_layout(s, row, col, &l)?;
    wb.set_input(0, 1, 2, "11")?;
    wb.refresh_pivot(s, row, col)?;
    // A second one on the data sheet, then deleted.
    let (s2, r2, c2) = wb.insert_pivot(
        "Sheet1!$A$1:$D$9",
        0,
        PivotDestination::Cell {
            sheet: 0,
            row: 30,
            col: 8,
        },
    )?;
    wb.delete_pivot(s2, r2, c2)?;
    // And a third one kept on the data sheet.
    let (s3, r3, c3) = wb.insert_pivot(
        "Sheet1!$A$1:$D$9",
        0,
        PivotDestination::Cell {
            sheet: 0,
            row: 40,
            col: 8,
        },
    )?;
    let mut l = wb.pivot_layout(s3, r3, c3).context("pivot layout")?;
    l.toggle_field(product);
    l.toggle_field(revenue);
    wb.set_pivot_layout(s3, r3, c3, &l)?;
    Ok(())
}

/// Everything Kantor writes into a sheet, on the data at row `top`.
fn kantor_features_at(wb: &mut Workbook, sheet: u32, top: u32) -> Result<()> {
    let data = CellRange::from_corners((top, 0), (top + ROWS.len() as u32, 3));
    wb.create_table(sheet, data, true, "TableStyleMedium2")?;
    for kind in [
        ChartType::Column,
        ChartType::StackedBar,
        ChartType::Line,
        ChartType::Pie,
        ChartType::StackedArea,
        ChartType::Scatter,
    ] {
        let key = wb.insert_chart(sheet, data, kind)?;
        let mut spec = wb.chart_spec(key).context("chart spec")?;
        spec.title = "Sales & costs".into();
        spec.legend = sheets_engine::LegendPos::Bottom;
        wb.set_chart(key, spec)?;
    }
    let units = format!("C{}:C{}", top + 2, top + 1 + ROWS.len() as u32);
    let names = format!("A{}:A{}", top + 2, top + 1 + ROWS.len() as u32);
    let rev = format!("D{}:D{}", top + 2, top + 1 + ROWS.len() as u32);
    let f = |spec| CfRuleDef {
        spec,
        format: CfFormat::PRESETS[0].1,
        stop_if_true: false,
    };
    for (range, spec) in [
        (
            &units,
            CfSpec::CellValue {
                op: CfCompare::GreaterThan,
                value: "7".into(),
                value2: String::new(),
            },
        ),
        (
            &units,
            CfSpec::CellValue {
                op: CfCompare::Between,
                value: "2".into(),
                value2: "4".into(),
            },
        ),
        (
            &names,
            CfSpec::Text {
                op: CfTextOp::Contains,
                text: "st".into(),
            },
        ),
        (&names, CfSpec::Duplicates { unique: false }),
        (
            &rev,
            CfSpec::Dates {
                period: CfPeriod::Today,
            },
        ),
        (
            &rev,
            CfSpec::TopBottom {
                bottom: false,
                rank: 10,
                percent: true,
            },
        ),
        (&rev, CfSpec::Average { below: true }),
        (&names, CfSpec::Blanks { not: false }),
        (
            &units,
            CfSpec::Formula {
                formula: format!("=$C{}>5", top + 2),
            },
        ),
    ] {
        wb.add_cf_rule(sheet, range, &f(spec))?;
    }
    let plain = |spec| CfRuleDef {
        spec,
        format: CfFormat::default(),
        stop_if_true: false,
    };
    wb.add_cf_rule(
        sheet,
        &rev,
        &plain(CfSpec::DataBar {
            color: 0x638EC6,
            gradient: true,
        }),
    )?;
    wb.add_cf_rule(
        sheet,
        &units,
        &plain(CfSpec::ColorScale {
            colors: vec![0xF8696B, 0xFFEB84, 0x63BE7B],
        }),
    )?;
    wb.add_cf_rule(
        sheet,
        &rev,
        &plain(CfSpec::IconSet {
            set: "3Arrows".into(),
            reverse: false,
            show_value: true,
        }),
    )?;
    // Data validation: a whole number with messages, a list.
    let whole = Validation {
        kind: ValidationType::Whole,
        operator: ValidationOperator::Between,
        allow_blank: true,
        dropdown: true,
        prompt: Some(Message {
            title: "Quantity".into(),
            text: "A whole number from 1 to 10".into(),
        }),
        error: Some((
            ErrorStyle::Warning,
            Message {
                title: "Check".into(),
                text: "1 to 10 & no more".into(),
            },
        )),
        formula1: Some(validation_operand(ValidationType::Whole, "1")?),
        formula2: Some(validation_operand(ValidationType::Whole, "10")?),
        ranges: Vec::new(),
    };
    let list = Validation {
        kind: ValidationType::List,
        operator: ValidationOperator::Between,
        allow_blank: true,
        dropdown: true,
        prompt: None,
        error: Some((ErrorStyle::Stop, Message::default())),
        formula1: Some(list_source("Open, Closed, Pending")?),
        formula2: None,
        ranges: Vec::new(),
    };
    wb.set_validation(
        sheet,
        &[CellRange::from_corners((top + 1, 2), (top + 8, 2))],
        whole,
    )?;
    wb.set_validation(
        sheet,
        &[CellRange::from_corners((top + 1, 5), (top + 8, 5))],
        list,
    )?;
    // Page setup, panes, hidden rows and columns, row heights, widths.
    wb.set_print_settings(
        sheet,
        SheetPrint {
            setup: PageSetup {
                paper: Paper::FOLIO,
                landscape: true,
                scale: 100,
                fit: Some((1, 0)),
                over_then_down: true,
                first_page_number: Some(3),
                black_and_white: false,
            },
            margins: Margins::NARROW,
            options: PrintOptions {
                grid_lines: true,
                headings: true,
                horizontal_centered: true,
                vertical_centered: false,
            },
            header_footer: HeaderFooter {
                odd_header: "&L&F&C&\"Arial,Bold\"Sales & Co&R&D &T".into(),
                odd_footer: "&CPage &P of &N".into(),
                first_header: "First".into(),
                even_footer: "Even".into(),
                different_first: true,
                different_odd_even: true,
                scale_with_doc: true,
                align_with_margins: true,
                ..HeaderFooter::default()
            },
            // Kantor doesn't edit manual page breaks (set_print_settings
            // keeps the sheet's own).
            row_breaks: Vec::new(),
            col_breaks: Vec::new(),
            print_area: vec![CellRange::from_corners((top, 0), (top + 8, 3))],
            title_rows: Some((top, top)),
            title_cols: Some((0, 0)),
        },
    )?;
    wb.set_frozen(sheet, top + 1, 1)?;
    wb.hide_rows(sheet, top + 20, top + 22)?;
    wb.set_columns_hidden(sheet, 10, 11, true)?;
    wb.set_row_heights(sheet, top + 1, top + 3, RowHeight::Points(30.0))?;
    wb.set_row_heights(sheet, top + 4, top + 4, RowHeight::Auto(15.0))?;
    wb.set_column_widths(sheet, &[(0, 20.0), (3, 14.5)])?;
    Ok(())
}

/// A sheet AutoFilter and a table's filter, with values and conditions,
/// and a sort.
fn filters(wb: &mut Workbook) -> Result<()> {
    let data = CellRange::from_corners((0, 0), (ROWS.len() as u32, 3));
    wb.sort_range(
        0,
        data,
        &[sheets_engine::SortKey {
            col: 3,
            descending: true,
        }],
        true,
    )?;
    wb.toggle_filter(0, data)?;
    wb.set_column_filter(
        0,
        &FilterScope::Sheet,
        0,
        Some(FilterKind::Values {
            values: vec!["East".into(), "West".into()],
            blanks: false,
            dates: Vec::new(),
        }),
    )?;
    let top = 20;
    add_data(wb, 0, top)?;
    let range = CellRange::from_corners((top, 0), (top + ROWS.len() as u32, 3));
    let name = wb.create_table(0, range, true, "TableStyleLight9")?;
    wb.set_column_filter(
        0,
        &FilterScope::Table(name),
        1,
        Some(FilterKind::Values {
            values: vec!["Apples".into()],
            blanks: true,
            dates: Vec::new(),
        }),
    )?;
    wb.delete_columns(0, 6, 6)?;
    Ok(())
}

/// Refresh, re-lay-out, delete and insert pivots in file 21.
fn pivot_edits(wb: &mut Workbook) -> Result<()> {
    let mut found = Vec::new();
    for s in 0..wb.sheet_names().len() as u32 {
        for r in 0..40 {
            for c in 0..12 {
                if let Some(p) = wb.pivot_at(s, r, c)
                    && !found
                        .iter()
                        .any(|(fs, n): &(u32, String)| *fs == s && *n == p.name)
                {
                    found.push((s, p.name.clone()));
                    let (r0, c0) = (p.area.top, p.area.left);
                    wb.refresh_pivot(s, r0, c0)?;
                }
            }
        }
    }
    anyhow::ensure!(found.len() >= 3, "pivots found: {found:?}");
    // Delete the last pivot found, re-lay-out the first.
    let (s, name) = found.last().unwrap().clone();
    let at = (0..40)
        .flat_map(|r| (0..12).map(move |c| (r, c)))
        .find(|&(r, c)| wb.pivot_at(s, r, c).is_some_and(|p| p.name == name))
        .context("pivot")?;
    wb.delete_pivot(s, at.0, at.1)?;
    let data = wb
        .sheet_names()
        .iter()
        .position(|n| n == "Data")
        .context("Data sheet")? as u32;
    let (ns, nr, nc) = wb.insert_pivot("Sales", data, PivotDestination::NewSheet)?;
    let mut l = wb.pivot_layout(ns, nr, nc).context("layout")?;
    l.toggle_field(0);
    if l.fields.len() > 3 {
        l.toggle_field(3);
    }
    wb.set_pivot_layout(ns, nr, nc, &l)?;
    Ok(())
}

// ---------------------------------------------------------------- Docs

/// Typing, a comment, headers and footers, a section break, a page field.
fn doc_edits(doc: &mut Document) -> Result<()> {
    let start = Selection::caret(Position::new(0, 0));
    doc.insert_text(start, "Kantor edit. ", None);
    let word = Selection {
        anchor: Position::new(0, 0),
        head: Position::new(0, 6),
    };
    doc.add_comment(word, "A comment from Kantor & co.", "Kantor")
        .context("add comment")?;
    let caret = Selection::caret(Position::new(0, 0));
    doc.set_hf_text(
        0,
        HfKind::Header,
        HfType::Default,
        "Header text",
        None,
        caret,
    );
    doc.set_hf_text(
        0,
        HfKind::Footer,
        HfType::Default,
        "Footer text",
        None,
        caret,
    );
    doc.set_title_page(0, true, caret);
    doc.set_hf_text(0, HfKind::Header, HfType::First, "First page", None, caret);
    doc.set_even_odd_headers(true, caret);
    doc.set_hf_text(0, HfKind::Footer, HfType::Even, "Even footer", None, caret);
    let end = doc.end();
    let after = doc.newline(Selection::caret(end), None);
    let after = doc.insert_text(after, "After the break.", None);
    doc.insert_section_break(after);
    let end = doc.end();
    let s = doc.insert_text(Selection::caret(end), "Page ", None);
    doc.insert_page_field(s, PageField::Page);
    Ok(())
}

/// Accept one tracked change, reject another, comment edits.
fn review_edits(doc: &mut Document) -> Result<()> {
    let changes = doc.changes();
    anyhow::ensure!(changes.len() >= 3, "changes: {changes:?}");
    let first = changes[0].clone();
    doc.accept_change(Selection::caret(first.start))
        .context("accept")?;
    let changes = doc.changes();
    let next = changes[0].clone();
    doc.reject_change(Selection::caret(next.start))
        .context("reject")?;
    let caret = Selection::caret(Position::new(0, 0));
    let comments = doc.comments().to_vec();
    if let Some(c) = comments.first() {
        doc.set_comment_text(&c.id, "Edited comment", caret);
    }
    let sel = Selection {
        anchor: Position::new(1, 0),
        head: Position::new(1, 4),
    };
    doc.add_comment(sel, "New comment", "Kantor");
    doc.set_hf_text(
        0,
        HfKind::Header,
        HfType::Default,
        "New header",
        None,
        caret,
    );
    let end = doc.end();
    doc.insert_text(Selection::caret(end), "\tTabbed", None);
    Ok(())
}

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

/// A Word-style document with tracked changes (insert, delete, formatting
/// and paragraph-mark changes), tab stops, a comment, a header and a footer.
fn review_docx() -> Result<Vec<u8>> {
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/></Relationships>"#;
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="character" w:styleId="CommentReference"><w:name w:val="annotation reference"/><w:rPr><w:sz w:val="16"/></w:rPr></w:style></w:styles>"#
    );
    let settings = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings {W_NS}><w:trackRevisions/><w:defaultTabStop w:val="720"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#
    );
    let d = r#"w:author="Reviewer" w:date="2026-11-01T10:00:00Z""#;
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly review</w:t></w:r></w:p><w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="2880"/><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:leader="dot" w:pos="9360"/><w:tab w:val="decimal" w:pos="7200"/></w:tabs></w:pPr><w:r><w:t>Item</w:t></w:r><w:r><w:tab/><w:t>Owner</w:t></w:r><w:r><w:tab/><w:t>Due</w:t></w:r><w:r><w:tab/><w:t>12.50</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">Sales grew </w:t></w:r><w:ins w:id="1" {d}><w:r><w:t xml:space="preserve">strongly </w:t></w:r></w:ins><w:r><w:t xml:space="preserve">in the </w:t></w:r><w:del w:id="2" {d}><w:r><w:delText xml:space="preserve">third </w:delText></w:r></w:del><w:r><w:t>quarter.</w:t></w:r></w:p><w:p><w:commentRangeStart w:id="0"/><w:r><w:t xml:space="preserve">Costs </w:t></w:r><w:commentRangeEnd w:id="0"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="0"/></w:r><w:ins w:id="3" {d}><w:r><w:t>fell</w:t></w:r></w:ins><w:del w:id="4" {d}><w:r><w:delText>rose</w:delText></w:r></w:del><w:r><w:t xml:space="preserve"> by </w:t></w:r><w:r><w:rPr><w:b/><w:rPrChange w:id="5" {d}><w:rPr/></w:rPrChange></w:rPr><w:t>4%</w:t></w:r><w:r><w:t>.</w:t></w:r></w:p><w:p><w:pPr><w:rPr><w:ins w:id="6" {d}/></w:rPr></w:pPr><w:ins w:id="7" {d}><w:r><w:t>An inserted paragraph.</w:t></w:r></w:ins></w:p><w:p><w:r><w:t>End.</w:t></w:r></w:p><w:sectPr><w:headerReference w:type="default" r:id="rId4"/><w:footerReference w:type="default" r:id="rId5"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:cols w:space="720"/></w:sectPr>"#
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W_NS}><w:body>{body}</w:body></w:document>"#
    );
    let comments = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments {W_NS}><w:comment w:id="0" w:author="Reviewer" w:date="2026-11-01T10:00:00Z" w:initials="R"><w:p><w:r><w:t>Check this figure.</w:t></w:r></w:p></w:comment></w:comments>"#
    );
    let header = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:hdr {W_NS}><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>Confidential</w:t></w:r></w:p></w:hdr>"#
    );
    let footer = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr {W_NS}><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#
    );
    zip_parts(&[
        ("[Content_Types].xml", ct),
        ("_rels/.rels", rels),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/document.xml", &document),
        ("word/styles.xml", &styles),
        ("word/settings.xml", &settings),
        ("word/comments.xml", &comments),
        ("word/header1.xml", &header),
        ("word/footer1.xml", &footer),
    ])
}

fn zip_parts(parts: &[(&str, &str)]) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut z = zip::ZipWriter::new(&mut buf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());
    for (name, text) in parts {
        z.start_file(*name, opts)?;
        z.write_all(text.as_bytes())?;
    }
    z.finish()?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------- Slides

/// Table cell edits, chart data edits, notes, new and duplicated slides.
fn slide_edits(p: &mut Presentation) -> Result<()> {
    let mut table = None;
    let mut chart = None;
    for i in 0..p.len() {
        for k in 0..p.slide(i).shapes.len() {
            if table.is_none() && p.table(i, k).is_some() {
                table = Some((i, k));
            }
            if chart.is_none() && p.slide(i).shapes[k].chart_part().is_some() {
                chart = Some((i, k));
            }
        }
    }
    if let Some((s, k)) = table {
        p.edit_table(
            s,
            k,
            &[
                TableOp::Text {
                    row: 1,
                    col: 0,
                    text: "Utara & co".into(),
                },
                TableOp::InsertRow {
                    row: 1,
                    below: true,
                },
                TableOp::Text {
                    row: 2,
                    col: 1,
                    text: "Two\nlines".into(),
                },
                TableOp::InsertCol {
                    col: 0,
                    right: true,
                },
                TableOp::Fill {
                    row: 0,
                    col: 1,
                    color: Some(Rgb([0x1F, 0x5F, 0xBF])),
                },
                TableOp::DeleteRow(3),
            ],
        )?;
    }
    if let Some((s, k)) = chart {
        let d = p.chart_data(s, k).context("chart data")?;
        let mut e: ChartData = d.clone();
        e.categories.push("Extra".into());
        for (n, series) in e.series.iter_mut().enumerate() {
            series.values.push(Some(9.5 + n as f64));
            series.name.push_str(" & plan");
        }
        if let Some(v) = e.series.first_mut().and_then(|s| s.values.first_mut()) {
            *v = None;
        }
        p.set_chart_data(s, k, &e)?;
    }
    for i in 0..p.len() {
        p.set_notes_text(i, &format!("Notes for slide {} <&>", i + 1))?;
    }
    let n = p.new_slide(Some(p.len() - 1))?;
    p.set_notes_text(n, "A new slide's notes")?;
    p.duplicate_slide(0)?;
    Ok(())
}
