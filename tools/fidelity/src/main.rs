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
use slides_engine::{ChartData, Inner, Presentation, Rgb, ShapeRef, TableOp, Xfrm};

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
    // Chart sheets: their charts restyled, tabs renamed, moved, hidden and
    // deleted, a deletion saved and undone (put back from the opened parts).
    let chart_sheets = corpus.join("xlsx/22_chart_sheets.xlsx");
    run.scenario("chart_sheets_edited.xlsx", |o| {
        let mut wb = Workbook::open(&chart_sheets)?;
        chart_sheet_edits(&mut wb, o)?;
        wb.save_as(o.join("chart_sheets_edited.xlsx"))?;
        Ok(Some(chart_sheets.clone()))
    });
    // Data labels and axis options, Move Chart both ways, copies of sheets
    // with charts, pictures and tables, chart sheet copies and tab colours.
    let charts = corpus.join("xlsx/20_charts.xlsx");
    run.scenario("chart_options_moves.xlsx", |o| {
        let mut wb = Workbook::open(&charts)?;
        chart_option_edits(&mut wb)?;
        wb.save_as(o.join("chart_options_moves.xlsx"))?;
        Ok(Some(charts.clone()))
    });
    let images = corpus.join("xlsx/14_charts_images.xlsx");
    run.scenario("sheet_copies_14_charts_images.xlsx", |o| {
        let mut wb = Workbook::open(&images)?;
        wb.copy_sheet(0, wb.tabs().len())?;
        wb.save_as(o.join("sheet_copies_14_charts_images.xlsx"))?;
        Ok(Some(images.clone()))
    });
    // Custom table styles: one of the file's modified, a new one with every
    // element formatted (default style, used by a table), saved twice.
    let styles = corpus.join("xlsx/24_table_styles.xlsx");
    run.scenario("table_styles_24.xlsx", |o| {
        let mut wb = Workbook::open(&styles)?;
        table_style_edits(&mut wb)?;
        wb.save_as(o.join("table_styles_24.xlsx"))?;
        wb.save_as(o.join("table_styles_24.xlsx"))?;
        Ok(Some(styles.clone()))
    });
    run.scenario("table_styles_new.xlsx", |o| {
        let mut wb = data_book()?;
        wb.create_table(
            0,
            CellRange::from_corners((0, 0), (ROWS.len() as u32, 3)),
            true,
            "TableStyleMedium2",
        )?;
        table_style_edits(&mut wb)?;
        wb.save_as(o.join("table_styles_new.xlsx"))?;
        Ok(None)
    });
    let tables = corpus.join("xlsx/13_tables.xlsx");
    run.scenario("sheet_copies_13_tables.xlsx", |o| {
        let mut wb = Workbook::open(&tables)?;
        wb.copy_sheet(0, 1)?;
        wb.save_as(o.join("sheet_copies_13_tables.xlsx"))?;
        Ok(Some(tables.clone()))
    });
    // Shapes: text edited (saved, undone, saved again), every preset
    // inserted on a sheet with a drawing and on one without, a copied
    // sheet's shapes.
    let shapes = corpus.join("xlsx/23_shapes.xlsx");
    run.scenario("shapes_edited.xlsx", |o| {
        let mut wb = Workbook::open(&shapes)?;
        let key = |wb: &Workbook, name: &str| {
            wb.drawings(0)
                .into_iter()
                .find(|d| d.name == name)
                .map(|d| d.key)
        };
        if let Some(k) = key(&wb, "Revenue box") {
            wb.set_shape_text(k, "Sales & costs\nUp <5%>")?;
        }
        if let Some(k) = key(&wb, "Notes box") {
            wb.set_shape_text(k, "")?;
        }
        wb.save_as(o.join("shapes_edited.xlsx"))?;
        if let Some(k) = key(&wb, "Dashed frame") {
            wb.set_shape_text(k, "One\n\nThree")?;
        }
        for (i, p) in std::iter::once(sheets_engine::ShapePreset::TextBox)
            .chain(sheets_engine::ShapePreset::SHAPES)
            .enumerate()
        {
            let k = wb.insert_shape(1, p, 2 + 6 * i as u32, 1)?;
            if wb.shape_text(k).is_some() {
                wb.set_shape_text(k, "Inserted")?;
            }
            wb.insert_shape(0, p, 30 + 6 * i as u32, 2)?;
        }
        wb.copy_sheet(0, 1)?;
        wb.save_as(o.join("shapes_edited.xlsx"))?;
        Ok(Some(shapes.clone()))
    });
    run.scenario("chart_sheet_copies.xlsx", |o| {
        let mut wb = Workbook::open(&chart_sheets)?;
        if let Some(sheets_engine::Tab::Chart(k)) = wb.tabs().first().copied() {
            wb.set_chart_sheet_tab_color(k, Some(sheets_engine::TabColor::Rgb(0xC00000)))?;
            let c = wb.copy_chart_sheet(k, 2)?;
            wb.set_chart_sheet_tab_color(
                c,
                Some(sheets_engine::TabColor::Theme { slot: 5, tint: 0.4 }),
            )?;
        }
        wb.save_as(o.join("chart_sheet_copies.xlsx"))?;
        Ok(Some(chart_sheets.clone()))
    });
    let pivots = corpus.join("xlsx/21_pivot_tables.xlsx");
    run.scenario("pivots_21_pivot_tables.xlsx", |o| {
        let mut wb = Workbook::open(&pivots)?;
        pivot_edits(&mut wb)?;
        wb.save_as(o.join("pivots_21_pivot_tables.xlsx"))?;
        Ok(Some(pivots.clone()))
    });
    let settings = corpus.join("xlsx/23_pivot_settings.xlsx");
    run.scenario("pivots_23_pivot_settings.xlsx", |o| {
        let mut wb = Workbook::open(&settings)?;
        pivot_settings_edits(&mut wb)?;
        wb.save_as(o.join("pivots_23_pivot_settings.xlsx"))?;
        Ok(Some(settings.clone()))
    });
    let slicers = corpus.join("xlsx/25_slicers.xlsx");
    run.scenario("slicers_25_slicers.xlsx", |o| {
        let mut wb = Workbook::open(&slicers)?;
        // Report's Region slicer: East and North; Data's Product slicer: Pears.
        wb.slicer_select(2, "Region", &[true, true, false])?;
        wb.slicer_select(1, "Product", &[false, true, false])?;
        wb.save_as(o.join("slicers_25_slicers.xlsx"))?;
        Ok(Some(slicers.clone()))
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
    // Fields updated (F9 over everything, then Update Table): a rebuilt
    // table of contents with new `_Toc` bookmarks and PAGEREF fields,
    // results replaced, a simple field given its end tag.
    let fields_src = corpus.join("docx/fields.docx");
    run.scenario("fields_updated.docx", |o| {
        let mut doc = Document::open(&fields_src)?;
        let cx = docs_engine::FieldContext {
            now: chrono::NaiveDate::from_ymd_opt(2026, 11, 15)
                .and_then(|d| d.and_hms_opt(9, 0, 0))
                .context("date")?,
            offset: chrono::FixedOffset::east_opt(7 * 3600).context("offset")?,
        };
        let mut engine = docs_pdf::Engine::new();
        let all = Selection {
            anchor: Position::new(0, 0),
            head: doc.end(),
        };
        doc.update_fields(all, &cx, &mut |d| docs_pdf::field_pages(d, &mut engine));
        let caret = Selection::caret(Position::new(0, 0));
        doc.update_tables_of_contents(caret, &cx, &mut |d| docs_pdf::field_pages(d, &mut engine));
        doc.save_as(&o.join("fields_updated.docx"))?;
        Ok(Some(fields_src.clone()))
    });
    // Floating objects edited: one moved, resized and rewrapped, an inline
    // picture made floating, a floating one put in line, a text box's text
    // replaced, one deleted.
    let floats_src = corpus.join("docx/floats.docx");
    run.scenario("floats_edited.docx", |o| {
        let mut doc = Document::open(&floats_src)?;
        let floats: Vec<(usize, usize, String)> = doc
            .paragraphs()
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                p.objects()
                    .filter_map(move |(at, o)| o.float().map(|f| (i, at, f.name.clone())))
            })
            .collect();
        let find = |n: &str| floats.iter().find(|f| f.2 == n).context("float");
        let (p, i, _) = find("Square picture")?;
        let mut at = doc.float_at(*p, *i).context("float")?.at;
        at.h = (
            docs_engine::float::RelH::Column,
            docs_engine::float::Place::Offset(190_500),
        );
        at.cx += 95_250;
        doc.set_float_placement(*p, *i, at);
        doc.set_layout_option(*p, *i, docs_engine::LayoutOption::Through);
        let (p, i, _) = find("Cropped picture")?;
        doc.set_layout_option(*p, *i, docs_engine::LayoutOption::Inline);
        doc.set_layout_option(*p, *i, docs_engine::LayoutOption::Behind);
        let (p, i, _) = find("Text Box 1")?;
        doc.set_text_box_text(*p, *i, "Edited\nin Kantor");
        let (p, i, _) = find("Arrow")?;
        doc.delete_float(*p, *i);
        doc.save_as(&o.join("floats_edited.docx"))?;
        Ok(Some(floats_src.clone()))
    });
    // Headers, footers and notes edited in place with formatting, a table
    // and a picture; a new footer in a later section, Link to Previous
    // off, distances; Insert Field, a bookmark, a cross-reference and an
    // edited field code.
    let report_src = corpus.join("docx/report.docx");
    run.scenario("parts_fields_edited.docx", |o| {
        use docs_engine::{HfKind, HfType, NoteKind, Toggle, XrefKind, XrefWhat};
        let mut doc = Document::open(&report_src)?;
        let cx = docs_engine::FieldContext {
            now: chrono::NaiveDate::from_ymd_opt(2026, 11, 16)
                .and_then(|d| d.and_hms_opt(9, 0, 0))
                .context("date")?,
            offset: chrono::FixedOffset::east_opt(7 * 3600).context("offset")?,
        };
        let mut engine = docs_pdf::Engine::new();
        let caret = Selection::caret(Position::new(1, 0));
        let (e, mut h) = doc
            .edit_hf(0, HfKind::Header, HfType::Default)
            .context("header")?;
        let c = h.insert_text(Selection::default(), "Draft ", None);
        h.toggle(
            Selection {
                anchor: Position::new(0, 0),
                head: Position::new(0, 5),
            },
            Toggle::Bold,
        );
        let end = h.end();
        let c2 = h.newline(Selection::caret(end), None);
        let c2 = h.insert_image(c2, png_2x1(), "logo.png")?;
        h.insert_table(c2, 1, 2).context("table")?;
        let _ = c;
        doc.finish_part(e, h, caret);
        let last = doc.sections().len() - 1;
        doc.set_hf_linked(last, HfKind::Footer, HfType::Default, false, caret);
        let (e, mut f) = doc
            .edit_hf(last, HfKind::Footer, HfType::Default)
            .context("footer")?;
        let end = f.end();
        f.insert_field(
            Selection::caret(end),
            "DATE \\@ \"d MMMM yyyy\"",
            &cx,
            &mut |d| docs_pdf::field_pages(d, &mut engine),
        );
        doc.finish_part(e, f, caret);
        doc.set_hf_distances(0, 600, 500, caret);
        let (c, id) = doc.insert_note(NoteKind::Footnote, Selection::caret(Position::new(2, 4)));
        let (e, mut n) = doc.edit_note(NoteKind::Footnote, &id).context("note")?;
        let end = n.end();
        let a = n.insert_text(Selection::caret(end), "Source: board survey.", None);
        n.toggle(
            Selection {
                anchor: end,
                head: a.head,
            },
            Toggle::Italic,
        );
        let a = n.newline(a, None);
        n.insert_image(a, png_2x1(), "chart.png")?;
        doc.finish_part(e, n, c);
        let end = doc.end();
        let c = doc.insert_text(Selection::caret(end), "\nSee ", None);
        let headings = doc.xref_targets(&XrefKind::Heading);
        let target = headings.first().context("heading")?;
        let c = doc.insert_cross_reference(
            c,
            &XrefKind::Heading,
            target,
            XrefWhat::Text,
            true,
            &cx,
            &mut |d| docs_pdf::field_pages(d, &mut engine),
        );
        let c = doc.insert_cross_reference(
            c,
            &XrefKind::Heading,
            target,
            XrefWhat::PageNumber,
            true,
            &cx,
            &mut |d| docs_pdf::field_pages(d, &mut engine),
        );
        let c = doc.insert_field(c, "TITLE", &cx, &mut |d| {
            docs_pdf::field_pages(d, &mut engine)
        });
        doc.add_bookmark(
            Selection {
                anchor: Position::new(1, 0),
                head: Position::new(1, 5),
            },
            "Board",
        )
        .map_err(anyhow::Error::msg)?;
        let r = doc.insert_field(c, "REF Board \\h", &cx, &mut |d| {
            docs_pdf::field_pages(d, &mut engine)
        });
        let _ = r;
        let begin = doc
            .fields()
            .into_iter()
            .find(|f| f.kind() == "TITLE")
            .context("title field")?
            .begin;
        doc.set_field_code(begin, "AUTHOR", caret, &cx, &mut |d| {
            docs_pdf::field_pages(d, &mut engine)
        });
        doc.update_part_fields(caret, &cx);
        doc.save_as(&o.join("parts_fields_edited.docx"))?;
        Ok(Some(report_src.clone()))
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
    // Strict Open XML files (scripts/corpus/gen_strict.py), saved as
    // Transitional: checked against the Transitional files they were made from.
    let strict = corpus.join("strict");
    run.scenario("strict_dates.xlsx", |o| {
        let mut wb = Workbook::open(strict.join("dates.xlsx"))?;
        sheet_edits(&mut wb)?;
        wb.save_as(o.join("strict_dates.xlsx"))?;
        Ok(Some(corpus.join("xlsx/04_dates.xlsx")))
    });
    run.scenario("strict_report.docx", |o| {
        let mut doc = Document::open(&strict.join("report.docx"))?;
        doc_edits(&mut doc)?;
        doc.save_as(&o.join("strict_report.docx"))?;
        Ok(Some(corpus.join("docx/report.docx")))
    });
    run.scenario("strict_deck.pptx", |o| {
        let mut p = Presentation::open(&strict.join("deck.pptx"))?;
        slide_edits(&mut p)?;
        p.save_as(&o.join("strict_deck.pptx"))?;
        Ok(Some(corpus.join("pptx/deck.pptx")))
    });

    run.scenario("convert_groups.pptx", |o| {
        let src = corpus.join("pptx/groups.pptx");
        let mut p = Presentation::open(&src)?;
        for i in 0..p.len() {
            if let Some(d) =
                (0..p.slide(i).shapes.len()).find(|&k| p.slide(i).shapes[k].smartart().is_some())
            {
                p.convert_to_shapes((i, d))?;
            }
            if let Some(g) = (0..p.slide(i).shapes.len()).find(|&k| p.slide(i).shapes[k].is_group())
            {
                let inner = ShapeRef(i, g, Inner::of(&[0]).context("path")?);
                p.delete_shape(inner)?;
            }
        }
        p.save_as(&o.join("convert_groups.pptx"))?;
        Ok(Some(src))
    });

    // Graphic frames on the slide: moved, resized, typed into, copied
    // (charts and SmartArt with their parts), grouped.
    run.scenario("frames_edits.pptx", |o| {
        let src = corpus.join("pptx/frames.pptx");
        let mut p = Presentation::open(&src)?;
        let named = |p: &Presentation, s: usize, n: &str| {
            p.slide(s)
                .shapes
                .iter()
                .position(|x| x.name == n)
                .context(n.to_owned())
        };
        let t = named(&p, 0, "Plan")?;
        let x = p.slide(0).shapes[t].xfrm.context("table box")?;
        p.set_xfrm(
            (0, t),
            Xfrm {
                x: x.x + 100_000,
                cx: x.cx * 4 / 5,
                cy: x.cy * 6 / 5,
                ..x
            },
            false,
        )?;
        let cell = ShapeRef::from((0, t)).cell(2, 0);
        p.insert_text(cell, Selection::caret(Position::new(0, 5)), " & co", None)?;
        p.edit_table(
            0,
            t,
            &[
                TableOp::InsertCol {
                    col: 1,
                    right: true,
                },
                TableOp::DeleteRow(0),
            ],
        )?;
        let chart = named(&p, 1, "Sales")?;
        let note = named(&p, 1, "Note")?;
        let c = p.slide(1).shapes[chart].xfrm.context("chart box")?;
        p.set_xfrm(
            (1, chart),
            Xfrm {
                x: c.x + 50_000,
                cx: c.cx / 2,
                ..c
            },
            false,
        )?;
        let clip = p.copy_shapes(1, &[chart])?;
        p.paste_shapes(0, &clip)?;
        p.duplicate_shape((0, t))?;
        let groups = Presentation::open(&corpus.join("pptx/groups.pptx"))?;
        let d = (0..groups.slide(1).shapes.len())
            .find(|&k| groups.slide(1).shapes[k].smartart().is_some())
            .context("SmartArt")?;
        let clip = groups.copy_shapes(1, &[d])?;
        p.paste_shapes(2, &clip)?;
        p.group_shapes(1, &[chart, note])?;
        p.save_as(&o.join("frames_edits.pptx"))?;
        Ok(Some(src))
    });

    // Table row heights and cell lists, chart categories and series
    // inserted, removed and reordered anywhere; and the drawn chart types.
    run.scenario("structure_edits.pptx", |o| {
        let src = corpus.join("pptx/frames.pptx");
        let mut p = Presentation::open(&src)?;
        let named = |p: &Presentation, s: usize, n: &str| {
            p.slide(s)
                .shapes
                .iter()
                .position(|x| x.name == n)
                .context(n.to_owned())
        };
        let t = named(&p, 0, "Plan")?;
        let mut hs: Vec<i64> = p
            .table(0, t)
            .context("table")?
            .rows
            .iter()
            .map(|r| r.h)
            .collect();
        hs[1] += 250_000;
        p.edit_table(0, t, &[TableOp::RowHeights(hs)])?;
        let cell = ShapeRef::from((0, t)).cell(3, 1);
        p.toggle_list(cell, Selection::caret(Position::new(0, 0)), true)?;
        let chart = named(&p, 1, "Sales")?;
        p.insert_chart_category(1, chart, 1, "Q1.5")?;
        p.remove_chart_category(1, chart, 3)?;
        p.insert_chart_series(1, chart, 0, "West")?;
        p.move_chart_series(1, chart, 1, false)?;
        p.save_as(&o.join("structure_edits.pptx"))?;
        Ok(Some(src))
    });
    // SmartArt shapes moved and resized; SmartArt inside a group typed into.
    run.scenario("smartart_edits.pptx", |o| {
        let src = corpus.join("pptx/groups.pptx");
        let mut p = Presentation::open(&src)?;
        let d = (0..p.slide(1).shapes.len())
            .find(|&k| p.slide(1).shapes[k].smartart().is_some())
            .context("SmartArt")?;
        let ship = ShapeRef(1, d, Inner::of(&[2]).context("path")?);
        let x = p.placed(ship).context("placed")?.xfrm;
        p.set_xfrm(
            ship,
            Xfrm {
                x: x.x + 200_000,
                cy: x.cy * 2,
                ..x
            },
            false,
        )?;
        let g = (0..p.slide(2).shapes.len())
            .find(|&k| p.slide(2).shapes[k].is_group())
            .context("group")?;
        let plan = ShapeRef(2, g, Inner::of(&[0, 0]).context("path")?);
        p.insert_text(plan, Selection::caret(Position::new(0, 4)), " ahead", None)?;
        p.save_as(&o.join("smartart_edits.pptx"))?;
        Ok(Some(src))
    });
    run.scenario("chart_types.pptx", |o| {
        let src = corpus.join("pptx/charts.pptx");
        let mut p = Presentation::open(&src)?;
        let k = (0..p.slide(0).shapes.len())
            .find(|&k| p.slide(0).shapes[k].chart_part().is_some())
            .context("radar")?;
        p.insert_chart_series(0, k, 1, "Model C")?;
        p.save_as(&o.join("chart_types.pptx"))?;
        Ok(Some(src))
    });

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
    // Notes: the file's first one edited, its last one deleted, a new one
    // shown all the time.
    let cells = wb.note_cells(0);
    if let Some(&(row, col)) = cells.first() {
        let old = wb.note_at(0, row, col).map(|n| n.text).unwrap_or_default();
        wb.set_note(0, row, col, &format!("{old}\n(edited)"), "Kantor")?;
    }
    if cells.len() > 1
        && let Some(&(row, col)) = cells.last()
    {
        wb.delete_notes(0, CellRange::from_corners((row, col), (row, col)))?;
    }
    wb.set_note(0, 40, 6, "Kantor:\nA new note & <more>", "Kantor")?;
    wb.show_notes(0, CellRange::from_corners((40, 6), (40, 6)), true)?;
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

/// New Table Style with every element formatted (made the default and used
/// by the first table), a copy of a built-in style, and the file's own
/// styles modified.
fn table_style_edits(wb: &mut Workbook) -> Result<()> {
    use sheets_engine::format::{BorderLine, LineStyle};
    use sheets_engine::{TableElement, TableElementFormat};
    let name = wb.next_table_style_name();
    let mut spec = sheets_engine::TableStyleSpec::new(&name);
    let line = |style, color| {
        Some(BorderLine {
            style,
            color: Some(color),
        })
    };
    for (i, e) in TableElement::ALL.into_iter().enumerate() {
        let c = 0x203040 + 0x101010 * i as u32;
        spec.set_format(
            e,
            TableElementFormat {
                fill: Some(c),
                font: Some(0xFFFFFF - c),
                bold: Some(i % 2 == 0),
                italic: Some(i % 3 == 0),
                underline: Some(i % 4 == 0),
                strike: Some(i % 5 == 0),
                left: line(LineStyle::Thin, c),
                right: line(LineStyle::Medium, c),
                top: line(LineStyle::Dashed, c),
                bottom: line(LineStyle::Double, c),
                horizontal: line(LineStyle::Hair, c),
                vertical: line(LineStyle::DashDot, c),
            },
        );
        spec.set_size(e, 2);
    }
    wb.add_table_style(&spec, true)?;
    let copy = wb.duplicate_table_style_name("TableStyleMedium9");
    let mut medium = wb
        .table_style_spec("TableStyleMedium9")
        .context("built-in")?;
    medium.name = copy;
    wb.add_table_style(&medium, false)?;
    for old in wb.custom_table_styles() {
        if old == name || old == medium.name {
            continue;
        }
        let mut s = wb.table_style_spec(&old).context("style")?;
        s.name = format!("{old} & more");
        wb.modify_table_style(&old, &s, false)?;
    }
    if let Some(t) = wb.tables(0).first() {
        let mut o = wb.table_options(&t.name).context("table")?;
        o.style = Some(name);
        wb.set_table_options(&t.name, o)?;
    }
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
                comments: sheets_engine::PrintComments::AtEnd,
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
            // Manual page breaks, written as rowBreaks / colBreaks.
            row_breaks: vec![top + 4],
            col_breaks: vec![2],
            print_area: vec![CellRange::from_corners((top, 0), (top + 8, 3))],
            title_rows: Some((top, top)),
            title_cols: Some((0, 0)),
            hf_pictures: Vec::new(),
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
fn chart_sheet_edits(wb: &mut Workbook, out: &Path) -> Result<()> {
    use sheets_engine::{ChartType, LegendPos, Tab};
    let charts: Vec<usize> = wb
        .tabs()
        .into_iter()
        .filter_map(|t| match t {
            Tab::Chart(k) => Some(k),
            _ => None,
        })
        .collect();
    for (i, &k) in charts.iter().enumerate() {
        let Some(key) = wb.chart_sheet_drawings(k).first().map(|d| d.key) else {
            continue;
        };
        let Some(info) = wb.chart_info(key) else {
            continue;
        };
        let mut spec = info.spec;
        spec.title = format!("Edited {i}");
        spec.legend = LegendPos::Top;
        if !info.fixed && i == 0 {
            spec.kind = ChartType::Line;
        }
        wb.set_chart(key, spec)?;
    }
    if let [first, second, ..] = charts[..] {
        wb.rename_chart_sheet(first, "Renamed chart")?;
        wb.move_chart_sheet(first, wb.tabs().len())?;
        wb.delete_chart_sheet(second)?;
        wb.insert_rows(0, 0, 1)?;
        wb.save_as(out.join("chart_sheets_edited.step.xlsx"))?;
        wb.undo()?; // the row
        wb.undo()?; // the deletion
        wb.set_chart_sheet_hidden(first, true)?;
    }
    Ok(())
}

/// Data labels and axis options on every chart of sheet 1, the first moved
/// to a new chart sheet, the second to a chart sheet and back, and a new
/// chart moved to a new sheet.
fn chart_option_edits(wb: &mut Workbook) -> Result<()> {
    use sheets_engine::{AxisOptions, ChartPlace, DataLabels, DisplayUnits, LabelPos};
    let keys: Vec<_> = wb.drawings(0).iter().map(|d| d.key).collect();
    for key in &keys {
        let Some(info) = wb.chart_info(*key) else {
            continue;
        };
        if info.fixed {
            continue;
        }
        let mut spec = info.spec.clone();
        let pos = info.label_positions.first().copied();
        spec.labels = vec![
            DataLabels {
                value: true,
                category: true,
                percent: info.percent_labels,
                pos,
                num_fmt: Some("#,##0.0".into()),
                ..DataLabels::default()
            };
            info.series.len()
        ];
        let axis = AxisOptions {
            min: Some(1.0),
            max: Some(10000.0),
            log_base: Some(10.0),
            reverse: true,
            num_fmt: Some("0.0".into()),
            units: DisplayUnits::Thousands,
            ..AxisOptions::default()
        };
        if info.y_value {
            spec.y_axis = axis;
        } else if info.x_value {
            spec.x_axis = axis;
        }
        if info.has_axes && !info.x_value {
            spec.x_axis.reverse = true;
        }
        if let Some(LabelPos::OutsideEnd) = pos {
            spec.y_axis.major = Some(10.0);
            spec.y_axis.log_base = None;
        }
        wb.set_chart(*key, spec)?;
    }
    let movable: Vec<_> = keys
        .iter()
        .copied()
        .filter(|k| wb.chart_info(*k).is_some())
        .collect();
    if let [first, second, ..] = movable[..] {
        wb.move_chart(first, ChartPlace::NewSheet(wb.next_chart_sheet_name()))?;
        if let sheets_engine::MovedChart::ChartSheet(k) =
            wb.move_chart(second, ChartPlace::NewSheet("Moved twice".into()))?
            && let Some(d) = wb.chart_sheet_drawings(k).first()
        {
            wb.move_chart(d.key, ChartPlace::ObjectIn(0))?;
        }
    }
    let data = CellRange::from_corners((60, 0), (60 + ROWS.len() as u32, 3));
    add_data(wb, 0, 60)?;
    let key = wb.insert_chart(0, data, ChartType::Column)?;
    wb.move_chart(key, ChartPlace::NewSheet("New chart".into()))?;
    Ok(())
}

/// Grouping, a calculated field, Show Values As, field settings, a style,
/// a pivot sharing the cache and a sheet copy on the settings fixture.
fn pivot_settings_edits(wb: &mut Workbook) -> Result<()> {
    use sheets_engine::{AxisEntry, PivotFieldFilter, PivotSort, ShowAs};
    wb.refresh_all_pivots()?;
    let s = wb
        .sheet_names()
        .iter()
        .position(|n| n == "Settings")
        .context("Settings sheet")? as u32;
    let data = wb
        .sheet_names()
        .iter()
        .position(|n| n == "Data")
        .context("Data sheet")? as u32;
    // A new pivot on the same data shares the cache: Product rows, Price
    // as % of Grand Total, Units by month and year, banded Medium 9.
    let (ns, nr, nc) = wb.insert_pivot("Data!$A$1:$E$13", data, PivotDestination::NewSheet)?;
    let mut l = wb.pivot_layout(ns, nr, nc).context("new pivot layout")?;
    let field = |l: &sheets_engine::PivotLayout, n: &str| l.fields.iter().position(|f| f.name == n);
    let product = field(&l, "Product").context("Product")?;
    let price = field(&l, "Price").context("Price")?;
    let units = field(&l, "Units").context("Units")?;
    l.rows = vec![AxisEntry::Field(product)];
    l.add_value(price);
    l.add_value(units);
    l.values[0].show_as = ShowAs::PercentOfTotal;
    l.values[1].show_as = ShowAs::RankDescending;
    l.values[1].base_field = Some(product);
    l.fields[product].settings.sort = PivotSort::Descending;
    l.fields[product].settings.sort_by_value = Some(1);
    l.fields[product].settings.filter = Some(PivotFieldFilter {
        kind: "captionNotBeginsWith".into(),
        value_field: None,
        value1: "Pe".into(),
        value2: String::new(),
        top: true,
    });
    l.style.name = Some("PivotStyleMedium9".into());
    l.style.banded_rows = true;
    wb.set_pivot_layout(ns, nr, nc, &l)?;
    wb.set_pivot_calculated_field(ns, nr, nc, "Double", "=Revenue*2")?;
    // Group the products by hand, and copy the Settings sheet.
    wb.group_pivot_items(ns, nr, nc, product, &["Apples".into(), "Plums".into()])?;
    wb.copy_sheet(s, 99)?;
    Ok(())
}

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
    // Tab stops (Tabs dialog / ruler) and the default interval.
    let end = doc.end();
    let s = doc.newline(Selection::caret(end), None);
    let s = doc.insert_text(s, "Contents\t12", None);
    doc.edit_tab_stops(
        s,
        &docs_engine::tabs::TabEdit {
            set: vec![
                docs_engine::TabStop::new(
                    9000,
                    docs_engine::TabKind::Right,
                    docs_engine::TabLeader::Dot,
                ),
                docs_engine::TabStop::new(
                    1000,
                    docs_engine::TabKind::Bar,
                    docs_engine::TabLeader::None,
                ),
            ],
            ..Default::default()
        },
    );
    doc.edit_tab_stops(
        Selection::caret(Position::new(0, 0)),
        &docs_engine::tabs::TabEdit {
            clear_all: true,
            set: vec![docs_engine::TabStop::new(
                2000,
                docs_engine::TabKind::Decimal,
                docs_engine::TabLeader::Hyphen,
            )],
            ..Default::default()
        },
    );
    doc.set_default_tab(567, caret);
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
/// Groups and SmartArt: move a group, edit text and fill inside it, group
/// two loose shapes, edit SmartArt text and fill (none if the file has none).
fn group_edits(p: &mut Presentation) -> Result<()> {
    for i in 0..p.len() {
        let groups: Vec<usize> = (0..p.slide(i).shapes.len())
            .filter(|&k| p.slide(i).shapes[k].is_group())
            .collect();
        if let Some(&g) = groups.first() {
            let x = p.slide(i).shapes[g].xfrm.context("group box")?;
            p.set_xfrm(
                (i, g),
                Xfrm {
                    x: x.x + 100_000,
                    ..x
                }
                .with_degrees(15.0),
                false,
            )?;
            let child = ShapeRef(i, g, Inner::of(&[0]).context("path")?);
            if p.text_paras(child).is_some() {
                p.insert_text(
                    child,
                    Selection::caret(Position::new(0, 0)),
                    "In a group: ",
                    None,
                )?;
            }
            if p.shape(child).is_some_and(|s| s.paintable()) {
                p.set_fill(child, slides_engine::Paint::Rgb(Rgb([0x22, 0x88, 0x44])))?;
            }
            let c = p.placed(child).context("child")?.xfrm;
            p.set_xfrm(
                child,
                Xfrm {
                    y: c.y + 200_000,
                    ..c
                },
                false,
            )?;
        }
        if let Some(&g) = groups.get(1) {
            p.ungroup((i, g))?;
            p.regroup(i)?;
        }
        let loose: Vec<usize> = (0..p.slide(i).shapes.len())
            .filter(|&k| {
                let s = &p.slide(i).shapes[k];
                s.ph.is_none() && !s.is_group() && s.movable() && s.xfrm.is_some()
            })
            .collect();
        if loose.len() >= 2 {
            p.group_shapes(i, &loose[..2])?;
        }
        if let Some(d) =
            (0..p.slide(i).shapes.len()).find(|&k| p.slide(i).shapes[k].smartart().is_some())
        {
            let n = p.slide(i).shapes[d]
                .smartart()
                .map_or(0, |dd| dd.group.children.len());
            for k in 0..n {
                let r = ShapeRef(i, d, Inner::of(&[k]).context("path")?);
                if p.smartart_text_editable(r) && p.text_paras(r).is_some() {
                    p.insert_text(r, Selection::caret(Position::new(0, 0)), "Edited ", None)?;
                    p.set_fill(r, slides_engine::Paint::Scheme("accent2".into()))?;
                    break;
                }
            }
        }
    }
    Ok(())
}

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
    group_edits(p)?;
    for i in 0..p.len() {
        p.set_notes_text(i, &format!("Notes for slide {} <&>", i + 1))?;
    }
    let n = p.new_slide(Some(p.len() - 1))?;
    p.set_notes_text(n, "A new slide's notes")?;
    // The first slide Kantor can duplicate (slides with charts can't be).
    let dup = (0..p.len())
        .find(|&i| p.slide(i).shapes.iter().all(|s| s.chart_part().is_none()))
        .unwrap_or(0);
    p.duplicate_slide(dup)?;
    Ok(())
}

/// A valid 2×1 RGB PNG (synthetic): stored deflate blocks, no zlib crate.
fn png_2x1() -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    }
    let raw = [0u8, 200, 40, 40, 40, 90, 200];
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01, 1];
    z.extend((raw.len() as u16).to_le_bytes());
    z.extend((!(raw.len() as u16)).to_le_bytes());
    z.extend(raw);
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(2u32.to_be_bytes());
    ihdr.extend(1u32.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
