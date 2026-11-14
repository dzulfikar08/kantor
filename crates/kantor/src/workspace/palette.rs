//! The command palette (Ctrl+K): commands, the theme picker and the font picker
//! share one panel, like Voltrus Tools' palette modes.

use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::fuzzy;
use kantor_theme::catalog::{self, ThemeEntry};
use kantor_ui::components::kbd;
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use kantor_ui::tokens::Elevation;

use super::Workspace;
use super::doc::CommonFormat;
use crate::actions::*;
use crate::theme;
use sheets_ui::{BorderPreset, CalcMode, Format, GridCommand, NoteCommand, NumberFormat, VAlign};

/// A palette command.
pub(super) struct Command {
    title: &'static str,
    detail: &'static str,
    keys: &'static str,
    run: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
}

pub(super) const COMMANDS: &[Command] = &[
    Command {
        title: "New Spreadsheet",
        detail: "Sheets",
        keys: "Ctrl N",
        run: |ws, window, cx| ws.new_spreadsheet(&NewSpreadsheet, window, cx),
    },
    Command {
        title: "New Document",
        detail: "Docs",
        keys: "Ctrl Shift N",
        run: |ws, window, cx| ws.new_document(&NewDocument, window, cx),
    },
    Command {
        title: "Open File…",
        detail: ".xlsx, .docx, .pptx, .csv, .ods, .xls, .ppt",
        keys: "Ctrl O",
        run: |ws, window, cx| ws.open_file(&OpenFile, window, cx),
    },
    Command {
        title: "Copy MCP Setup for Claude Code",
        detail: "AI agents can edit open documents (kantor mcp)",
        keys: "",
        run: |ws, _, cx| ws.copy_mcp_setup(cx),
    },
    Command {
        title: "Save",
        detail: "Current document",
        keys: "Ctrl S",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.save(w, cx)),
    },
    Command {
        title: "Save As…",
        detail: "Current document",
        keys: "Ctrl Shift S",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.save_as(w, cx)),
    },
    Command {
        title: "Export as PDF…",
        detail: "Docs pages as laid out · Slides, one page per slide · Sheets as Excel prints",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| v.export_pdf_doc(w, cx));
            ws.with_slides(window, cx, |v, w, cx| v.export_pdf_doc(w, cx));
            ws.with_sheets_any(window, cx, |v, w, cx| v.export_pdf_doc(w, cx));
        },
    },
    Command {
        title: "Print…",
        detail: "Opens the pages as a PDF in the system viewer to print",
        keys: "Ctrl P",
        run: |ws, window, cx| ws.print(&crate::actions::Print, window, cx),
    },
    Command {
        title: "Print Notes Pages…",
        detail: "Slides: each slide above its speaker notes",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| {
                v.print_layout_doc(slides_pdf::PrintLayout::Notes, w, cx)
            })
        },
    },
    Command {
        title: "Print Handouts…",
        detail: "Slides: 3 slides per page with lines for notes (the Print menu has 1–9)",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| {
                v.print_layout_doc(slides_pdf::PrintLayout::Handouts(3), w, cx)
            })
        },
    },
    Command {
        title: "Edit Table or Chart Data…",
        detail: "Slides · the selected table's cells, rows and columns, or a chart's values",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.edit_object_doc(w, cx)),
    },
    Command {
        title: "Undo",
        detail: "Last edit",
        keys: "Ctrl Z",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.undo(w, cx)),
    },
    Command {
        title: "Redo",
        detail: "Last undone edit",
        keys: "Ctrl Shift Z",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.redo(w, cx)),
    },
    Command {
        title: "Copy",
        detail: "Selection (cells as tab-separated text)",
        keys: "Ctrl C",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.copy(w, cx)),
    },
    Command {
        title: "Cut",
        detail: "Copy and delete the selection",
        keys: "Ctrl X",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.cut(w, cx)),
    },
    Command {
        title: "Paste",
        detail: "Plain text, at the caret or selected cell",
        keys: "Ctrl V",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.paste(w, cx)),
    },
    Command {
        title: "Find",
        detail: "Text, cells or slides",
        keys: "Ctrl F",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.find(false, w, cx)),
    },
    Command {
        title: "Replace",
        detail: "Find and replace",
        keys: "Ctrl H",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.find(true, w, cx)),
    },
    Command {
        title: "Bold",
        detail: "Selection",
        keys: "Ctrl B",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| v.format(CommonFormat::Bold, w, cx))
        },
    },
    Command {
        title: "Italic",
        detail: "Selection",
        keys: "Ctrl I",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| v.format(CommonFormat::Italic, w, cx))
        },
    },
    Command {
        title: "Underline",
        detail: "Selection",
        keys: "Ctrl U",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::Underline, w, cx)
            })
        },
    },
    Command {
        title: "Strikethrough",
        detail: "Selected cells",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Strikethrough, w, cx)
            })
        },
    },
    Command {
        title: "Align Left",
        detail: "Selection or paragraphs",
        keys: "",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::AlignLeft, w, cx)
            })
        },
    },
    Command {
        title: "Align Center",
        detail: "Selection or paragraphs",
        keys: "",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::AlignCenter, w, cx)
            })
        },
    },
    Command {
        title: "Align Right",
        detail: "Selection or paragraphs",
        keys: "",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::AlignRight, w, cx)
            })
        },
    },
    Command {
        title: "Format as General",
        detail: "Number format",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::General), w, cx)
            })
        },
    },
    Command {
        title: "Format as Number",
        detail: "Number format · 1,234.50",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::Number), w, cx)
            })
        },
    },
    Command {
        title: "Format as Percent",
        detail: "Number format · 12.50%",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::Percent), w, cx)
            })
        },
    },
    Command {
        title: "Format as Rupiah",
        detail: "Number format · Rp1,250,000",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::CurrencyIdr), w, cx)
            })
        },
    },
    Command {
        title: "Format as Dollars",
        detail: "Number format · $1,234.50",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::CurrencyUsd), w, cx)
            })
        },
    },
    Command {
        title: "Format as Date",
        detail: "Number format · 2026-11-07",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::Date), w, cx)
            })
        },
    },
    Command {
        title: "Fill: Light Yellow",
        detail: "Sheets · cell background",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Fill(Some(0xFFF2CC)), w, cx)
            })
        },
    },
    Command {
        title: "Fill: Light Blue",
        detail: "Sheets · cell background",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Fill(Some(0xDDEBF7)), w, cx)
            })
        },
    },
    Command {
        title: "Fill: Light Green",
        detail: "Sheets · cell background",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Fill(Some(0xE2EFDA)), w, cx)
            })
        },
    },
    Command {
        title: "Fill: Light Grey",
        detail: "Sheets · cell background",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Fill(Some(0xF2F2F2)), w, cx)
            })
        },
    },
    Command {
        title: "No Fill",
        detail: "Sheets · cell background",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Fill(None), w, cx)
            })
        },
    },
    Command {
        title: "Text Colour: Red",
        detail: "Sheets",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::FontColor(Some(0xC00000)), w, cx)
            })
        },
    },
    Command {
        title: "Text Colour: Blue",
        detail: "Sheets",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::FontColor(Some(0x0070C0)), w, cx)
            })
        },
    },
    Command {
        title: "Text Colour: Green",
        detail: "Sheets",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::FontColor(Some(0x00B050)), w, cx)
            })
        },
    },
    Command {
        title: "Text Colour: Automatic",
        detail: "Sheets",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::FontColor(None), w, cx)
            })
        },
    },
    Command {
        title: "Increase Font Size",
        detail: "Sheets · one point larger",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::FontSize(1), w, cx)
            })
        },
    },
    Command {
        title: "Decrease Font Size",
        detail: "Sheets · one point smaller",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::FontSize(-1), w, cx)
            })
        },
    },
    Command {
        title: "All Borders",
        detail: "Sheets · every edge of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Borders(BorderPreset::All), w, cx)
            })
        },
    },
    Command {
        title: "Outside Borders",
        detail: "Sheets · outline of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Borders(BorderPreset::Outline), w, cx)
            })
        },
    },
    Command {
        title: "No Borders",
        detail: "Sheets · remove borders",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Borders(BorderPreset::None), w, cx)
            })
        },
    },
    Command {
        title: "Wrap Text",
        detail: "Sheets · toggle",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.format_doc(Format::Wrap, w, cx))
        },
    },
    Command {
        title: "Merge & Center / Unmerge",
        detail: "Sheets · selected cells",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.format_doc(Format::Merge, w, cx))
        },
    },
    Command {
        title: "Align Top",
        detail: "Sheets · vertical alignment",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::VAlign(VAlign::Top), w, cx)
            })
        },
    },
    Command {
        title: "Align Middle",
        detail: "Sheets · vertical alignment",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::VAlign(VAlign::Center), w, cx)
            })
        },
    },
    Command {
        title: "Align Bottom",
        detail: "Sheets · vertical alignment",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::VAlign(VAlign::Bottom), w, cx)
            })
        },
    },
    Command {
        title: "Sort A to Z",
        detail: "Sheets · by the active cell's column (smallest to largest, oldest to newest)",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::SortAscending, w, cx)
            })
        },
    },
    Command {
        title: "Sort Z to A",
        detail: "Sheets · by the active cell's column, descending",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::SortDescending, w, cx)
            })
        },
    },
    Command {
        title: "Custom Sort…",
        detail: "Sheets · sort by several columns",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::CustomSort, w, cx)
            })
        },
    },
    Command {
        title: "Filter",
        detail: "Sheets · filter buttons on or off for the data or table",
        keys: "Ctrl Shift L",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::ToggleFilter, w, cx)
            })
        },
    },
    Command {
        title: "Clear Filters",
        detail: "Sheets · show every row the sheet's filters hide",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::ClearFilters, w, cx)
            })
        },
    },
    Command {
        title: "Reapply Filters",
        detail: "Sheets · filter again after edits",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::ReapplyFilters, w, cx)
            })
        },
    },
    Command {
        title: "Insert Row Above",
        detail: "Sheets · rows of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::InsertRowsAbove, w, cx)
            })
        },
    },
    Command {
        title: "Insert Row Below",
        detail: "Sheets · rows of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::InsertRowsBelow, w, cx)
            })
        },
    },
    Command {
        title: "Delete Rows",
        detail: "Sheets · rows of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::DeleteRows, w, cx)
            })
        },
    },
    Command {
        title: "Insert Column Left",
        detail: "Sheets · columns of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::InsertColumnsLeft, w, cx)
            })
        },
    },
    Command {
        title: "Insert Column Right",
        detail: "Sheets · columns of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::InsertColumnsRight, w, cx)
            })
        },
    },
    Command {
        title: "Delete Columns",
        detail: "Sheets · columns of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::DeleteColumns, w, cx)
            })
        },
    },
    Command {
        title: "Autofit Column Width",
        detail: "Sheets · fit the selected columns to their content",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::AutofitColumns, w, cx)
            })
        },
    },
    Command {
        title: "Row Height…",
        detail: "Sheets · set the selected rows' height in points",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::RowHeight, w, cx)
            })
        },
    },
    Command {
        title: "Autofit Row Height",
        detail: "Sheets · fit the selected rows to their content",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::AutofitRows, w, cx)
            })
        },
    },
    Command {
        title: "Standard Column Width",
        detail: "Sheets · reset the selected columns",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::ResetColumnWidth, w, cx)
            })
        },
    },
    Command {
        title: "Freeze Top Row",
        detail: "Sheets · keep row 1 in view",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::FreezeTopRow, w, cx)
            })
        },
    },
    Command {
        title: "Freeze First Column",
        detail: "Sheets · keep column A in view",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::FreezeFirstColumn, w, cx)
            })
        },
    },
    Command {
        title: "Freeze Panes",
        detail: "Sheets · rows above and columns left of the active cell",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::FreezePanes, w, cx)
            })
        },
    },
    Command {
        title: "Unfreeze Panes",
        detail: "Sheets",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::Unfreeze, w, cx)
            })
        },
    },
    Command {
        title: "Hide Rows",
        detail: "Sheets · hide the selected rows",
        keys: "Ctrl 9",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::HideRows, w, cx)
            })
        },
    },
    Command {
        title: "Unhide Rows",
        detail: "Sheets · show hidden rows in the selection (not filtered ones)",
        keys: "Ctrl Shift 9",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::UnhideRows, w, cx)
            })
        },
    },
    Command {
        title: "Hide Columns",
        detail: "Sheets · hide the selected columns",
        keys: "Ctrl 0",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::HideColumns, w, cx)
            })
        },
    },
    Command {
        title: "Unhide Columns",
        detail: "Sheets · show hidden columns in the selection",
        keys: "Ctrl Shift 0",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::UnhideColumns, w, cx)
            })
        },
    },
    Command {
        title: "Select All Cells",
        detail: "Sheets",
        keys: "Ctrl A",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::SelectAll, w, cx)
            })
        },
    },
    Command {
        title: "Fill Down",
        detail: "Sheets · copy the top row down the selection",
        keys: "Ctrl D",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::FillDown, w, cx)
            })
        },
    },
    Command {
        title: "Fill Right",
        detail: "Sheets · copy the left column across the selection",
        keys: "Ctrl R",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.grid_command(GridCommand::FillRight, w, cx)
            })
        },
    },
    Command {
        title: "Calculate Now",
        detail: "Sheets · recalculate every formula",
        keys: "F9",
        run: |ws, window, cx| ws.with_sheets_any(window, cx, |v, w, cx| v.calculate_doc(w, cx)),
    },
    Command {
        title: "Automatic Calculation",
        detail: "Sheets · recalculate after every edit",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets_any(window, cx, |v, w, cx| {
                v.set_calc_mode_doc(CalcMode::Automatic, w, cx)
            })
        },
    },
    Command {
        title: "Manual Calculation",
        detail: "Sheets · recalculate only with F9 (for big workbooks)",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets_any(window, cx, |v, w, cx| {
                v.set_calc_mode_doc(CalcMode::Manual, w, cx)
            })
        },
    },
    Command {
        title: "New Conditional Formatting Rule…",
        detail: "Sheets · highlight cells, top/bottom, data bars, colour scales, icon sets or a formula",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.new_cf_rule_doc(w, cx)),
    },
    Command {
        title: "Manage Conditional Formatting Rules…",
        detail: "Sheets · edit, delete and reorder the rules of the selection or the sheet",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.manage_cf_doc(w, cx)),
    },
    Command {
        title: "Clear Conditional Formatting from Selected Cells",
        detail: "Sheets · remove the rules that touch the selection",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.clear_cf_doc(false, w, cx)),
    },
    Command {
        title: "Clear Conditional Formatting from Entire Sheet",
        detail: "Sheets · remove every rule of the active sheet",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.clear_cf_doc(true, w, cx)),
    },
    Command {
        title: "Insert Chart",
        detail: "Sheets · a column chart of the selection or the data around the active cell (other types in the toolbar's chart menu)",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.insert_chart_doc(sheets_engine::ChartType::Column, w, cx)
            })
        },
    },
    Command {
        title: "Insert Line Chart",
        detail: "Sheets · a line chart of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.insert_chart_doc(sheets_engine::ChartType::Line, w, cx)
            })
        },
    },
    Command {
        title: "Insert Pie Chart",
        detail: "Sheets · a pie chart of the selection",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.insert_chart_doc(sheets_engine::ChartType::Pie, w, cx)
            })
        },
    },
    Command {
        title: "Chart Panel",
        detail: "Sheets · type, title, legend, axis titles, series colours and data of the selected chart",
        keys: "",
        run: |ws, window, cx| ws.with_sheets_any(window, cx, |v, w, cx| v.chart_options_doc(w, cx)),
    },
    Command {
        title: "Format as Table…",
        detail: "Sheets · make the selection or the data around the active cell a table with a style",
        keys: "Ctrl T",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.format_as_table_doc(w, cx)),
    },
    Command {
        title: "Table Design",
        detail: "Sheets · style, header and total rows, banding, totals and name of the table at the active cell",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.table_design_doc(w, cx)),
    },
    Command {
        title: "Convert Table to Range",
        detail: "Sheets · keep the cells, remove the table at the active cell",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.convert_table_doc(w, cx)),
    },
    Command {
        title: "Page Setup…",
        detail: "Sheets · paper, orientation, scaling, margins, header and footer, print area and titles",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.page_setup_doc(w, cx)),
    },
    Command {
        title: "Set Print Area",
        detail: "Sheets · print only the selected cells of this sheet",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.set_print_area_doc(false, w, cx))
        },
    },
    Command {
        title: "Clear Print Area",
        detail: "Sheets · print the whole used range of this sheet again",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.set_print_area_doc(true, w, cx))
        },
    },
    Command {
        title: "Insert PivotTable…",
        detail: "Sheets · summarise the table, selection or data around the active cell",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.insert_pivot_doc(w, cx)),
    },
    Command {
        title: "PivotTable Fields",
        detail: "Sheets · show or hide the field list of the PivotTable at the active cell",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.pivot_fields_doc(w, cx)),
    },
    Command {
        title: "Delete PivotTable",
        detail: "Sheets · remove the PivotTable at the active cell",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.delete_pivot_doc(w, cx)),
    },
    Command {
        title: "Refresh PivotTable",
        detail: "Sheets · recompute the PivotTable at the active cell from its data",
        keys: "Alt F5",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.refresh_pivots_doc(false, w, cx))
        },
    },
    Command {
        title: "Refresh All PivotTables",
        detail: "Sheets · recompute every PivotTable in the workbook",
        keys: "Ctrl Alt F5",
        run: |ws, window, cx| {
            ws.with_sheets_any(window, cx, |v, w, cx| v.refresh_pivots_doc(true, w, cx))
        },
    },
    Command {
        title: "Pick From Validation List",
        detail: "Sheets · open the active cell's data validation drop-down",
        keys: "Alt Down",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.open_validation_list_doc(w, cx))
        },
    },
    Command {
        title: "Data Validation…",
        detail: "Sheets · allow only a list, numbers, dates, times, text length or a formula; input message and error alert",
        keys: "",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.data_validation_doc(w, cx)),
    },
    Command {
        title: "Circle Invalid Data",
        detail: "Sheets · ring cells whose values break their data validation",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.circle_invalid_doc(true, w, cx))
        },
    },
    Command {
        title: "Clear Validation Circles",
        detail: "Sheets · data validation",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| v.circle_invalid_doc(false, w, cx))
        },
    },
    Command {
        title: "Toggle Sidebar",
        detail: "Open documents, recent files",
        keys: "Ctrl Shift E",
        run: |ws, window, cx| ws.toggle_sidebar(&ToggleSidebar, window, cx),
    },
    Command {
        title: "Toggle Inspector",
        detail: "Cell and workbook details",
        keys: "Ctrl Shift I",
        run: |ws, window, cx| ws.toggle_inspector(&ToggleInspector, window, cx),
    },
    Command {
        title: "Split Right",
        detail: "New pane beside",
        keys: "Ctrl \\",
        run: |ws, window, cx| ws.split_right(&SplitRight, window, cx),
    },
    Command {
        title: "Split Down",
        detail: "New pane below",
        keys: "Ctrl Alt \\",
        run: |ws, window, cx| ws.split_down(&SplitDown, window, cx),
    },
    Command {
        title: "Close Pane",
        detail: "Or the tab, if it has one pane",
        keys: "Ctrl W",
        run: |ws, window, cx| ws.close_pane(&ClosePane, window, cx),
    },
    Command {
        title: "Close Tab",
        detail: "All its panes",
        keys: "Ctrl Shift W",
        run: |ws, window, cx| ws.close_active_tab(&CloseTab, window, cx),
    },
    Command {
        title: "Next Tab",
        detail: "",
        keys: "Ctrl Tab",
        run: |ws, window, cx| ws.next_tab(&NextTab, window, cx),
    },
    Command {
        title: "Previous Tab",
        detail: "",
        keys: "Ctrl Shift Tab",
        run: |ws, window, cx| ws.prev_tab(&PrevTab, window, cx),
    },
    Command {
        title: "Go Home",
        detail: "Start screen",
        keys: "Ctrl Shift H",
        run: |ws, window, cx| ws.go_home(&GoHome, window, cx),
    },
    Command {
        title: "Heading 1",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 1",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_style_doc(Some("Heading1"), w, cx)
            })
        },
    },
    Command {
        title: "Heading 2",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 2",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_style_doc(Some("Heading2"), w, cx)
            })
        },
    },
    Command {
        title: "Heading 3",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 3",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_style_doc(Some("Heading3"), w, cx)
            })
        },
    },
    Command {
        title: "Normal Text",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 0",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.set_style_doc(None, w, cx)),
    },
    Command {
        title: "Justify",
        detail: "Docs · paragraphs",
        keys: "Ctrl J",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_align_doc(docs_engine::Align::Justify, w, cx)
            })
        },
    },
    Command {
        title: "Bulleted List",
        detail: "Docs · lists",
        keys: "Ctrl Shift 8",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.toggle_list_doc(docs_engine::ListKind::Bullet, w, cx)
            })
        },
    },
    Command {
        title: "Numbered List",
        detail: "Docs · lists",
        keys: "Ctrl Shift 7",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.toggle_list_doc(docs_engine::ListKind::Numbered, w, cx)
            })
        },
    },
    Command {
        title: "Insert Table",
        detail: "Docs · 3 × 3 (toolbar grid for other sizes)",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.insert_table_doc(3, 3, w, cx)),
    },
    Command {
        title: "Insert Row Above",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::RowAbove, w, cx)
            })
        },
    },
    Command {
        title: "Insert Row Below",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::RowBelow, w, cx)
            })
        },
    },
    Command {
        title: "Insert Column Left",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::ColumnLeft, w, cx)
            })
        },
    },
    Command {
        title: "Insert Column Right",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::ColumnRight, w, cx)
            })
        },
    },
    Command {
        title: "Delete Row",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::DeleteRow, w, cx)
            })
        },
    },
    Command {
        title: "Delete Column",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::DeleteColumn, w, cx)
            })
        },
    },
    Command {
        title: "Delete Table",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::DeleteTable, w, cx)
            })
        },
    },
    Command {
        title: "Insert Picture…",
        detail: "Docs · PNG or JPEG",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.insert_image_doc(w, cx)),
    },
    Command {
        title: "Page Setup…",
        detail: "Docs · paper size, orientation, margins",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.toggle_page_menu(None, w, cx)),
    },
    Command {
        title: "Header and Footer…",
        detail: "Docs · text, page numbers, different first page, odd and even pages",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.open_hf_menu(docs_engine::HfKind::Header, None, w, cx)
            })
        },
    },
    Command {
        title: "Add Page Numbers",
        detail: "Docs · \"Page 1 of 9\" centred in the footer",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.add_page_numbers_doc(w, cx)),
    },
    Command {
        title: "Insert Page Number Field",
        detail: "Docs · the page number at the caret",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.insert_page_number_doc(w, cx)),
    },
    Command {
        title: "Insert Section Break (Next Page)",
        detail: "Docs · a new section with its own page setup, headers and footers",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| v.insert_section_break_doc(w, cx))
        },
    },
    Command {
        title: "Track Changes",
        detail: "Docs · turn on or off: your typing and deletions are marked for review",
        keys: "Ctrl Shift E",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| v.toggle_track_changes_doc(w, cx))
        },
    },
    Command {
        title: "Insert Footnote",
        detail: "Docs · a numbered note at the bottom of the page",
        keys: "Ctrl Alt F",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.insert_note_doc(docs_engine::NoteKind::Footnote, w, cx)
            })
        },
    },
    Command {
        title: "Insert Endnote",
        detail: "Docs · a numbered note at the end of the document",
        keys: "Ctrl Alt D",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.insert_note_doc(docs_engine::NoteKind::Endnote, w, cx)
            })
        },
    },
    Command {
        title: "Edit Footnote or Endnote",
        detail: "Docs · the note whose number is at the caret",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.edit_note_doc(w, cx)),
    },
    Command {
        title: "New Comment",
        detail: "Docs · on the selection or the word at the caret",
        keys: "Ctrl Alt M",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.new_comment_doc(w, cx)),
    },
    Command {
        title: "Show or Hide Comments",
        detail: "Docs · the comment cards beside the pages",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, _, cx| v.toggle_comments_doc(cx)),
    },
    Command {
        title: "Accept Tracked Change",
        detail: "Docs · the insertion or deletion at the caret, then the next one",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.accept_change_doc(w, cx)),
    },
    Command {
        title: "Reject Tracked Change",
        detail: "Docs · the insertion or deletion at the caret, then the next one",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.reject_change_doc(w, cx)),
    },
    Command {
        title: "Accept All Tracked Changes",
        detail: "Docs · insertions, deletions, paragraph marks and formatting",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.resolve_all_doc(true, w, cx)),
    },
    Command {
        title: "Reject All Tracked Changes",
        detail: "Docs · back to the text before the changes",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.resolve_all_doc(false, w, cx)),
    },
    Command {
        title: "Next Tracked Change",
        detail: "Docs · select the next insertion or deletion",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, _, cx| v.next_change_doc(true, cx)),
    },
    Command {
        title: "Previous Tracked Change",
        detail: "Docs · select the previous insertion or deletion",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, _, cx| v.next_change_doc(false, cx)),
    },
    Command {
        title: "Paper Size: A4",
        detail: "Docs · page size, 21 × 29.7 cm",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_paper_doc(docs_ui::PaperSize::A4, w, cx)
            })
        },
    },
    Command {
        title: "Paper Size: Letter",
        detail: "Docs · page size, 21.59 × 27.94 cm",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_paper_doc(docs_ui::PaperSize::Letter, w, cx)
            })
        },
    },
    Command {
        title: "Paper Size: Legal",
        detail: "Docs · page size, 21.59 × 35.56 cm",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_paper_doc(docs_ui::PaperSize::Legal, w, cx)
            })
        },
    },
    Command {
        title: "Paper Size: A5",
        detail: "Docs · page size, 14.8 × 21 cm",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_paper_doc(docs_ui::PaperSize::A5, w, cx)
            })
        },
    },
    Command {
        title: "Paper Size: F4 / Folio",
        detail: "Docs · page size, 21.5 × 33 cm",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_paper_doc(docs_ui::PaperSize::F4, w, cx)
            })
        },
    },
    Command {
        title: "Orientation: Portrait",
        detail: "Docs · page setup",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_orientation_doc(docs_ui::Orientation::Portrait, w, cx)
            })
        },
    },
    Command {
        title: "Orientation: Landscape",
        detail: "Docs · page setup",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_orientation_doc(docs_ui::Orientation::Landscape, w, cx)
            })
        },
    },
    Command {
        title: "Margins: Normal",
        detail: "Docs · 2.54 cm all sides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_margins_doc(docs_ui::MarginPreset::Normal, w, cx)
            })
        },
    },
    Command {
        title: "Margins: Narrow",
        detail: "Docs · 1.27 cm all sides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_margins_doc(docs_ui::MarginPreset::Narrow, w, cx)
            })
        },
    },
    Command {
        title: "Margins: Moderate",
        detail: "Docs · 2.54 cm top/bottom, 1.91 cm sides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_margins_doc(docs_ui::MarginPreset::Moderate, w, cx)
            })
        },
    },
    Command {
        title: "Margins: Wide",
        detail: "Docs · 2.54 cm top/bottom, 5.08 cm sides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_margins_doc(docs_ui::MarginPreset::Wide, w, cx)
            })
        },
    },
    Command {
        title: "Spelling…",
        detail: "Sheets · check the spelling of the text cells, from the active cell or in the selection",
        keys: "F7",
        run: |ws, window, cx| ws.with_sheets(window, cx, |v, w, cx| v.spelling_doc(w, cx)),
    },
    Command {
        title: "New Note / Edit Note",
        detail: "Sheets · write a note on the active cell (Review > Notes)",
        keys: "Shift+F2",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.note_command(NoteCommand::Edit, w, cx)
            })
        },
    },
    Command {
        title: "Delete Note",
        detail: "Sheets · delete the notes of the selected cells",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.note_command(NoteCommand::Delete, w, cx)
            })
        },
    },
    Command {
        title: "Show/Hide Note",
        detail: "Sheets · keep the active cell's note shown, or show it only on hover",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.note_command(NoteCommand::ShowHide, w, cx)
            })
        },
    },
    Command {
        title: "Show All Notes",
        detail: "Sheets · show every note, or only on hover again",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.note_command(NoteCommand::ShowAll, w, cx)
            })
        },
    },
    Command {
        title: "Next Note",
        detail: "Sheets · go to the next cell with a note",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.note_command(NoteCommand::Next, w, cx)
            })
        },
    },
    Command {
        title: "Previous Note",
        detail: "Sheets · go to the previous cell with a note",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.note_command(NoteCommand::Previous, w, cx)
            })
        },
    },
    Command {
        title: "Check Spelling as You Type",
        detail: "Docs, Slides · underline misspelled words in red",
        keys: "",
        run: |_, _, cx| kantor_ui::spelling::toggle(cx),
    },
    Command {
        title: "Toggle Ruler",
        detail: "Docs · show or hide the ruler",
        keys: "",
        run: |_, _, cx| docs_ui::toggle_ruler(cx),
    },
    Command {
        title: "Zoom In",
        detail: "Docs · page zoom",
        keys: "Ctrl =",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, _, cx| {
                v.zoom_by(kantor_ui::metrics::ZOOM_STEP, cx)
            })
        },
    },
    Command {
        title: "Zoom Out",
        detail: "Docs · page zoom",
        keys: "Ctrl -",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, _, cx| {
                v.zoom_by(1.0 / kantor_ui::metrics::ZOOM_STEP, cx)
            })
        },
    },
    Command {
        title: "Actual Size",
        detail: "Docs · page zoom 100%",
        keys: "Ctrl 0",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, _, cx| v.set_zoom(1.0, cx)),
    },
    Command {
        title: "New Presentation",
        detail: "Slides",
        keys: "Ctrl Alt N",
        run: |ws, window, cx| ws.new_presentation(&NewPresentation, window, cx),
    },
    Command {
        title: "New Slide",
        detail: "Slides",
        keys: "Ctrl M",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.new_slide_doc(w, cx)),
    },
    Command {
        title: "Duplicate Slide",
        detail: "Slides",
        keys: "Ctrl D",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.duplicate_slide_doc(w, cx)),
    },
    Command {
        title: "Delete Slide",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.delete_slide_doc(w, cx)),
    },
    Command {
        title: "Move Slide Up",
        detail: "Slides",
        keys: "Ctrl Up",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.move_slide_doc(true, w, cx)),
    },
    Command {
        title: "Move Slide Down",
        detail: "Slides",
        keys: "Ctrl Down",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.move_slide_doc(false, w, cx)),
    },
    Command {
        title: "Duplicate Shape",
        detail: "Slides · the selected shape",
        keys: "Ctrl D",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.duplicate_doc(w, cx)),
    },
    Command {
        title: "Bulleted List",
        detail: "Slides · Tab / Shift Tab change the level",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| v.toggle_list_doc(false, w, cx))
        },
    },
    Command {
        title: "Numbered List",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.toggle_list_doc(true, w, cx)),
    },
    Command {
        title: "Shape Fill…",
        detail: "Slides · theme colours, no fill",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, _, cx| v.open_paint_menu_doc(false, cx))
        },
    },
    Command {
        title: "Shape Outline…",
        detail: "Slides · colour and width",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, _, cx| v.open_paint_menu_doc(true, cx))
        },
    },
    Command {
        title: "Toggle Speaker Notes",
        detail: "Slides · the notes pane under the slide",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.toggle_notes_doc(w, cx)),
    },
    Command {
        title: "Insert Text Box",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.insert_text_box_doc(w, cx)),
    },
    Command {
        title: "Insert Rectangle",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| {
                v.insert_shape_doc(slides_engine::Geom::Rect, w, cx)
            })
        },
    },
    Command {
        title: "Insert Ellipse",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| {
                v.insert_shape_doc(slides_engine::Geom::Ellipse, w, cx)
            })
        },
    },
    Command {
        title: "Insert Picture…",
        detail: "Slides · PNG or JPEG",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.insert_image_doc(w, cx)),
    },
    Command {
        title: "Present",
        detail: "Slides · from the first slide",
        keys: "F5",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.present_doc(true, w, cx)),
    },
    Command {
        title: "Present From This Slide",
        detail: "Slides",
        keys: "Shift F5",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.present_doc(false, w, cx)),
    },
    Command {
        title: "AI Settings",
        detail: "Provider, API key, model · off by default",
        keys: "",
        run: |ws, window, cx| ws.open_ai_settings(&kantor_ui::ai::OpenAiSettings, window, cx),
    },
    Command {
        title: "Ask AI for a Formula…",
        detail: "Sheets · describe it in English or Indonesian",
        keys: "",
        run: |ws, window, cx| ws.ask_for_formula(&kantor_ui::ai::AskForFormula, window, cx),
    },
    Command {
        title: "Explain This Formula",
        detail: "Sheets · AI, active cell",
        keys: "",
        run: |ws, window, cx| ws.explain_formula(&kantor_ui::ai::ExplainFormula, window, cx),
    },
    Command {
        title: "Rewrite / Translate Selection…",
        detail: "Docs · AI: Indonesian ↔ English, formal, shorter, grammar",
        keys: "",
        run: |ws, window, cx| ws.rewrite_selection(&kantor_ui::ai::RewriteSelection, window, cx),
    },
    Command {
        title: "Choose Theme…",
        detail: "Omarchy, Ghostty",
        keys: "",
        run: |ws, window, cx| ws.open_palette(PaletteMode::themes(), window, cx),
    },
    Command {
        title: "Choose UI Font…",
        detail: "SF Pro, Inter, …",
        keys: "",
        run: |ws, window, cx| ws.open_palette(PaletteMode::fonts(cx), window, cx),
    },
    Command {
        title: "Turn Autosave On or Off",
        detail: "Saves .xlsx/.docx/.pptx files with a name after you stop typing",
        keys: "",
        run: |ws, _, cx| ws.toggle_autosave(cx),
    },
    Command {
        title: "Recover Unsaved Work…",
        detail: "Documents left unsaved when Kantor closed unexpectedly",
        keys: "",
        run: |ws, window, cx| ws.reoffer_recovery(window, cx),
    },
    Command {
        title: "Check for Updates…",
        detail: concat!(
            "Kantor ",
            env!("CARGO_PKG_VERSION"),
            " · sends only its version and OS"
        ),
        keys: "",
        run: |ws, _, cx| ws.check_for_updates(true, cx),
    },
    Command {
        title: "Restart to Update",
        detail: "Install the downloaded update and reopen your documents",
        keys: "",
        run: |ws, window, cx| ws.restart_to_update(window, cx),
    },
    Command {
        title: "Quit Kantor",
        detail: "",
        keys: "Ctrl Q",
        run: |_, _, cx| cx.quit(),
    },
];

/// What the palette is listing.
pub(super) enum PaletteMode {
    Commands,
    Themes(Vec<ThemeEntry>),
    /// `None` = automatic font choice.
    Fonts(Vec<Option<String>>),
}

impl PaletteMode {
    pub(super) fn themes() -> Self {
        Self::Themes(catalog::all())
    }

    pub(super) fn fonts(cx: &App) -> Self {
        let mut fonts = vec![None];
        fonts.extend(kantor_ui::fonts::installed(cx).iter().cloned().map(Some));
        Self::Fonts(fonts)
    }

    pub(super) fn names(&self) -> Vec<String> {
        match self {
            Self::Commands => COMMANDS.iter().map(|c| c.title.to_owned()).collect(),
            Self::Themes(t) => t.iter().map(|e| e.name.clone()).collect(),
            Self::Fonts(f) => f
                .iter()
                .map(|f| f.clone().unwrap_or_else(|| "Automatic".to_owned()))
                .collect(),
        }
    }

    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            Self::Commands => "Type a command…",
            Self::Themes(_) => "Search themes…",
            Self::Fonts(_) => "Search fonts…",
        }
    }

    pub(super) fn row_height(&self) -> Pixels {
        match self {
            Self::Themes(_) => PALETTE_THEME_ROW_HEIGHT,
            _ => PALETTE_ROW_HEIGHT,
        }
    }

    pub(super) fn noun(&self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Themes(_) => "themes",
            Self::Fonts(_) => "fonts",
        }
    }
}

pub(super) struct Palette {
    mode: PaletteMode,
    input: Entity<InputState>,
    /// Indices into the mode's items, best match first.
    matches: Vec<usize>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl Workspace {
    pub(super) fn toggle_palette(
        &mut self,
        _: &TogglePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.close_palette(true, window, cx);
        } else {
            self.open_palette(PaletteMode::Commands, window, cx);
        }
    }

    pub(super) fn open_palette(
        &mut self,
        mode: PaletteMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = mode.placeholder();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(&input, window, Self::on_palette_input);
        input.update(cx, |i, cx| i.focus(window, cx));
        let matches: Vec<usize> = (0..mode.names().len()).collect();
        // Start on the active theme/font so the picker opens where you are.
        let selected = match &mode {
            PaletteMode::Commands => 0,
            PaletteMode::Themes(t) => {
                let active = &theme::current(cx).theme;
                t.iter().position(|e| &e.id == active).unwrap_or(0)
            }
            PaletteMode::Fonts(f) => {
                let active = &theme::current(cx).ui_font;
                f.iter().position(|x| x == active).unwrap_or(0)
            }
        };
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_item(selected, ScrollStrategy::Center);
        self.palette = Some(Palette {
            mode,
            input,
            matches,
            selected,
            scroll,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Redraw just the palette (typing, moving the highlight); the rest of
    /// the window keeps its last frame.
    pub(super) fn notify_palette(&self, cx: &mut Context<Self>) {
        if let Some(c) = &self.chrome {
            c.palette.update(cx, |_, cx| cx.notify());
        }
    }

    pub(super) fn on_palette_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                if let Some(p) = self.palette.as_mut() {
                    let names = p.mode.names();
                    p.matches = fuzzy::rank(&query, names.iter().map(String::as_str));
                    p.selected = 0;
                    p.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                self.notify_palette(cx);
            }
            InputEvent::PressEnter { .. } => self.run_selected(window, cx),
            _ => {}
        }
    }

    /// Close the palette; `restore` undoes theme/font previews (Esc, click outside).
    pub(super) fn close_palette(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.palette.take() else {
            return;
        };
        if restore && !matches!(p.mode, PaletteMode::Commands) {
            theme::restore(cx);
        }
        // Return focus to whatever the palette covered.
        self.activate(self.active, window, cx);
    }

    pub(super) fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(true, window, cx);
    }

    pub(super) fn palette_escape(
        &mut self,
        _: &input::Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_palette(true, window, cx);
    }

    pub(super) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        let n = p.matches.len() as isize;
        if n == 0 {
            return;
        }
        p.selected = (p.selected as isize + delta).rem_euclid(n) as usize;
        p.scroll.scroll_to_item(p.selected, ScrollStrategy::Center);
        self.preview_selected(cx);
        self.notify_palette(cx);
    }

    /// Live preview of the highlighted theme or font.
    pub(super) fn preview_selected(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_ref() else {
            return;
        };
        let Some(&ix) = p.matches.get(p.selected) else {
            return;
        };
        match &p.mode {
            PaletteMode::Commands => {}
            PaletteMode::Themes(t) => {
                let palette = t[ix].palette.clone();
                theme::preview_palette(&palette, cx);
            }
            PaletteMode::Fonts(f) => {
                let font = f[ix].clone();
                theme::preview_font(font.as_deref(), cx);
            }
        }
    }

    pub(super) fn palette_up(&mut self, _: &PaletteUp, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_move(-1, cx);
    }

    pub(super) fn palette_down(&mut self, _: &PaletteDown, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_move(1, cx);
    }

    /// Run/apply item `ix` of the current mode.
    pub(super) fn palette_pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.take() else {
            return;
        };
        match p.mode {
            PaletteMode::Commands => {
                self.activate(self.active, window, cx);
                if let Some(cmd) = COMMANDS.get(ix) {
                    (cmd.run)(self, window, cx);
                }
            }
            PaletteMode::Themes(t) => {
                if let Some(e) = t.into_iter().nth(ix) {
                    self.message = format!("Theme: {}", e.name).into();
                    theme::set_theme(e.id, cx);
                }
                self.activate(self.active, window, cx);
            }
            PaletteMode::Fonts(f) => {
                if let Some(font) = f.into_iter().nth(ix) {
                    self.message =
                        format!("Font: {}", font.as_deref().unwrap_or("Automatic")).into();
                    theme::set_font(font, cx);
                }
                self.activate(self.active, window, cx);
            }
        }
    }

    pub(super) fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .palette
            .as_ref()
            .and_then(|p| p.matches.get(p.selected).copied());
        match ix {
            Some(ix) => self.palette_pick(ix, window, cx),
            None => self.close_palette(true, window, cx),
        }
    }

    pub(super) fn render_palette_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        let (active_theme, active_font) = {
            let a = theme::current(cx);
            (a.theme.clone(), a.ui_font.clone())
        };
        let Some(p) = self.palette.as_ref() else {
            return Vec::new();
        };
        let row_h = p.mode.row_height();
        let automatic = match p.mode {
            PaletteMode::Fonts(_) => kantor_ui::fonts::automatic_ui_font(cx),
            _ => None,
        };
        let mut rows = Vec::new();
        for row in range {
            let Some(&ix) = p.matches.get(row) else {
                continue;
            };
            let selected = row == p.selected;
            let entity = entity.clone();
            let base = div()
                .id(("palette-row", ix))
                .w_full()
                .h(row_h)
                .px(SPACE_6)
                .flex()
                .items_center()
                .gap(SPACE_4)
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when_else(
                    selected,
                    |d| d.bg(t.selected),
                    |d| d.hover(|s| s.bg(t.hover)),
                )
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.palette_pick(ix, window, cx));
                });
            let check = |on: bool| {
                div()
                    .w(px(14.))
                    .text_size(FONT_SMALL)
                    .text_color(t.accent)
                    .child(if on { "✓" } else { "" })
            };
            let row_el = match &p.mode {
                PaletteMode::Commands if bench_variant() == 2 => base.child(
                    div()
                        .text_size(FONT_BODY)
                        .text_color(t.fg)
                        .child(COMMANDS[ix].title),
                ),
                PaletteMode::Commands => {
                    let cmd = &COMMANDS[ix];
                    base.child(div().text_size(FONT_BODY).text_color(t.fg).child(cmd.title))
                        .child(
                            div()
                                .flex_1()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_dim)
                                .child(cmd.detail),
                        )
                        .when(!cmd.keys.is_empty(), |d| d.child(kbd(cmd.keys, cx)))
                }
                PaletteMode::Themes(themes) => {
                    let e = &themes[ix];
                    let pal = &e.palette;
                    let swatch = h_flex()
                        .h(px(14.))
                        .w_full()
                        .rounded(px(3.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(t.border)
                        .child(
                            div()
                                .w(CONTROL_MD)
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgb(pal.background))
                                .text_color(rgb(pal.foreground))
                                .text_size(FONT_CAPTION)
                                .font_weight(FontWeight::MEDIUM)
                                .child("Ab"),
                        )
                        .children(
                            e.swatch()
                                .into_iter()
                                .map(|c| div().flex_1().h_full().bg(rgb(c))),
                        );
                    base.child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(SPACE_2)
                            .child(
                                h_flex()
                                    .gap(SPACE_3)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(FONT_BODY)
                                            .text_color(t.fg)
                                            .child(e.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(FONT_CAPTION)
                                            .text_color(t.fg_dim)
                                            .child(e.group.label()),
                                    )
                                    .child(check(e.id == active_theme)),
                            )
                            .child(swatch),
                    )
                }
                PaletteMode::Fonts(fonts) => {
                    let font = &fonts[ix];
                    let (label, family) = match font {
                        Some(f) => (f.clone(), f.clone()),
                        None => (
                            format!("Automatic ({})", automatic.as_deref().unwrap_or("system")),
                            automatic.clone().unwrap_or_default(),
                        ),
                    };
                    base.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(FONT_EMPHASIS)
                            .text_color(t.fg)
                            .when(!family.is_empty(), |d| d.font_family(family))
                            .child(label),
                    )
                    .child(check(font == &active_font))
                }
            };
            rows.push(row_el);
        }
        rows
    }

    pub(super) fn render_palette(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        let hints: Vec<Div> = [("↑↓", "navigate"), ("↵", "choose"), ("Esc", "close")]
            .into_iter()
            .map(|(keys, label)| h_flex().gap(SPACE_2).child(kbd(keys, cx)).child(label))
            .collect();
        let on_escape = cx.listener(Self::palette_escape);
        let rows_fn = cx.processor(|ws: &mut Self, range: std::ops::Range<usize>, _, cx| {
            ws.render_palette_rows(range, cx)
        });
        let palette = self.palette.as_ref()?;

        let count = palette.matches.len();
        let row_h = palette.mode.row_height();
        let visible = count.clamp(1, PALETTE_MAX_ROWS) as f32;
        let list = uniform_list("palette-list", count, rows_fn)
            .track_scroll(&palette.scroll)
            .w_full()
            .h(row_h * visible);

        let panel = v_flex()
            .id("palette")
            .key_context(PALETTE)
            .on_action(on_escape)
            .w(PALETTE_WIDTH)
            .mt(PALETTE_TOP)
            .rounded(RADIUS_XL)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border_strong)
            .when(bench_variant() != 3, |d| {
                d.shadow(t.shadow(Elevation::Modal))
            })
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(SPACE_4)
                    .border_b_1()
                    .border_color(t.border)
                    .child(Input::new(&palette.input).appearance(false)),
            )
            .child(div().p(SPACE_3).when_else(
                count == 0,
                |d| {
                    d.child(
                        div()
                            .h(PALETTE_ROW_HEIGHT)
                            .px(SPACE_6)
                            .flex()
                            .items_center()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_dim)
                            .child("No matches"),
                    )
                },
                |d| d.child(list),
            ))
            .child(
                h_flex()
                    .h(PALETTE_FOOTER_HEIGHT)
                    .px(SPACE_6)
                    .gap(SPACE_6)
                    .bg(t.chrome)
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(FONT_CAPTION)
                    .text_color(t.fg_muted)
                    .children(hints)
                    .child(div().flex_1().min_w_0().truncate().text_right().child(
                        match palette.mode {
                            PaletteMode::Commands => self.updates.about_line(),
                            _ => String::new(),
                        },
                    ))
                    .child(format!("{count} {}", palette.mode.noun())),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .when(bench_variant() != 3, |d| d.bg(t.scrim))
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.close_palette(true, window, cx));
                })
                .child(panel),
        )
    }
}

/// What `KANTOR_BENCH_PALETTE` leaves out of the palette while timing it.
/// 0 = nothing, 2 = row details and shortcut keys, 3 = shadow and backdrop.
static BENCH_VARIANT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn bench_variant() -> u8 {
    BENCH_VARIANT.load(std::sync::atomic::Ordering::Relaxed)
}

impl Workspace {
    /// `KANTOR_BENCH_PALETTE=1`: once the document has loaded, open the
    /// palette and time how long changes take to reach the screen: typing,
    /// moving the highlight, typing with parts of the rows left out, and
    /// full redraws. Prints the averages, then quits.
    pub(super) fn bench_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use std::cell::Cell;
        use std::rc::Rc;
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};
        enum Step {
            Type(&'static str),
            Move,
            Refresh,
        }
        cx.spawn_in(window, async move |this, cx| {
            let executor = cx.background_executor().clone();
            let pause = |ms| executor.timer(Duration::from_millis(ms));
            for _ in 0..600 {
                pause(200).await;
                let ready = this
                    .update(cx, |ws, cx| match ws.focused_view() {
                        Some(super::doc::DocView::Sheets(v)) => !v.read(cx).is_busy(),
                        _ => true,
                    })
                    .unwrap_or(false);
                if ready {
                    break;
                }
            }
            pause(1500).await;
            let _ = this.update_in(cx, |ws, window, cx| {
                ws.open_palette(PaletteMode::Commands, window, cx)
            });
            pause(800).await;
            // Each query keeps the list full (10 rows).
            let typing = || {
                ["i", "in", "ins", "inse", "inser", "insert", "s", "se", "sel", "e"]
                    .map(Step::Type)
                    .into_iter()
                    .collect::<Vec<_>>()
            };
            let cases: Vec<(&str, u8, Vec<Step>)> = vec![
                ("typing", 0, typing()),
                ("moving highlight", 0, (0..10).map(|_| Step::Move).collect()),
                ("typing, names only", 2, typing()),
                ("typing, no shadow", 3, typing()),
                ("full redraw", 0, (0..5).map(|_| Step::Refresh).collect()),
            ];
            for (label, variant, steps) in cases {
                BENCH_VARIANT.store(variant, Ordering::Relaxed);
                let mut times = Vec::new();
                let mut rows = 0;
                for step in steps {
                    let start = Instant::now();
                    let frames = Rc::new(Cell::new(0u8));
                    let _ = this.update_in(cx, |ws, window, cx| {
                        match step {
                            Step::Type(q) => {
                                if let Some(p) = ws.palette.as_mut() {
                                    p.input.update(cx, |i, cx| i.set_value(q, window, cx));
                                    let names = p.mode.names();
                                    p.matches = fuzzy::rank(q, names.iter().map(String::as_str));
                                    p.selected = 0;
                                }
                                ws.notify_palette(cx);
                            }
                            Step::Move => ws.palette_move(1, cx),
                            Step::Refresh => window.refresh(),
                        }
                        rows = ws.palette.as_ref().map_or(0, |p| p.matches.len());
                        let f = frames.clone();
                        window.on_next_frame(move |window, _| {
                            f.set(1);
                            window.on_next_frame(move |_, _| f.set(2));
                        });
                    });
                    while frames.get() < 2 && start.elapsed() < Duration::from_secs(5) {
                        pause(1).await;
                    }
                    if frames.get() == 2 {
                        times.push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                    pause(120).await;
                }
                times.sort_by(f64::total_cmp);
                match times.get(times.len() / 2) {
                    Some(median) => eprintln!(
                        "[bench] {label:<20} median {median:7.1} ms  (max {:7.1}, {} of them, {rows} matches)",
                        times.last().copied().unwrap_or_default(),
                        times.len(),
                    ),
                    None => eprintln!(
                        "[bench] {label:<20} no frames drawn (is the window visible?)"
                    ),
                }
            }
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
