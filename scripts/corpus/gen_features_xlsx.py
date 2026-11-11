#!/usr/bin/env python3
"""Generate the feature-rich part of the .xlsx corpus (tests/corpus/xlsx/13_*..18_*).

These files hold what Excel users put in workbooks beyond cells: tables, charts
and chart sheets, sparklines, x14 conditional formats, data validation,
comments, hyperlinks, defined names, print setup, protection, outlines,
images, a form control and every border and alignment option. They are
written with XlsxWriter (its output is close to Excel's own), from data made
up here, and dedicated to the public domain under CC0-1.0 like the rest of the
corpus (tests/corpus/README.md).

Next to the workbooks the script writes `features.tsv`: one line per feature,
with the part it lives in and an XML probe that must match in the original
and still match after Kantor opens and saves the file (and saves it again).
`crates/sheets_engine/tests/corpus.rs` runs the probes; features Kantor still
loses are listed in `known-gaps.tsv` with a reason.

Probe syntax: `a[@x=1]/b[@y=2]` matches an element `a` anywhere in the part
whose attributes `x` and `y` (local names) have those values, with a child `b`;
`[@x]` only asks for the attribute, `[text()=v]` for the text, and `//b` (in
place of `/b`) for a descendant. Part selectors: `pkg:<name>`, `styles`,
`workbook`, `sheet:<sheet name>` (worksheet or chart sheet), and
`rel:<sheet name>:<relationship kind>` (any part the sheet links to that way).

    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_features_xlsx.py           # write
    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_features_xlsx.py --check   # CI

The output is deterministic (fixed document dates; `--check` compares
unzipped parts, so zip timestamps and compression don't count).
"""

from __future__ import annotations

import datetime as dt
import io
import os
import struct
import sys
import tempfile
import zipfile
import zlib

import xlsxwriter

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUT = os.path.join(ROOT, "tests", "corpus", "xlsx")
CREATED = dt.datetime(2026, 11, 10, 12, 0, 0)
WANT_VERSION = "3.3.1"

# (file, feature, part selector, probe)
FEATURES: list[tuple[str, str, str, str]] = []


def feature(file: str, name: str, part: str, probe: str) -> None:
    FEATURES.append((file, name, part, probe))


def new_book(path: str) -> xlsxwriter.Workbook:
    wb = xlsxwriter.Workbook(path, {"in_memory": True})
    wb.set_properties({"title": os.path.basename(path), "author": "Kantor corpus", "created": CREATED})
    return wb


def png(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    """A solid-colour PNG, written by hand (no image library needed)."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


# ---------------------------------------------------------------- 13 tables

def f13_tables(path: str) -> None:
    f = "13_tables.xlsx"
    wb = new_book(path)
    head = wb.add_format({"bold": True, "font_color": "#FFFFFF", "bg_color": "#305496"})
    money = wb.add_format({"num_format": "#,##0.00"})

    ws = wb.add_worksheet("Sales")
    rows = [["Pump", 4, 120.5], ["Valve", 10, 15.25], ["Motor", 2, 480.0], ["Seal", 25, 2.75],
            ["Filter", 8, 9.9], ["Cable", 40, 1.2]]
    data = [[name, units, price, units * price] for name, units, price in rows]
    ws.add_table("A1:D8", {
        "name": "Sales",
        "style": "Table Style Medium 9",
        "total_row": True,
        "data": data,
        "columns": [
            {"header": "Item", "total_string": "Total", "header_format": head},
            {"header": "Units", "total_function": "sum"},
            {"header": "Price", "format": money},
            {"header": "Amount", "formula": "=[@Units]*[@Price]", "format": money,
             "total_function": "sum", "total_value": sum(r[3] for r in data)},
        ],
    })
    ws.write_formula("F1", "=SUM(Sales[Units])", None, sum(r[1] for r in rows))
    ws.write_formula("F2", "=MAX(Sales[Amount])", None, max(r[3] for r in data))
    ws.set_column("A:D", 12)

    lk = wb.add_worksheet("Lookup")
    lk.add_table("B2:C6", {
        "name": "Regions",
        "style": "Table Style Light 1",
        "autofilter": False,
        "banded_columns": True,
        "banded_rows": False,
        "first_column": True,
        "data": [["Jawa Timur", 0.11], ["Bali", 0.1], ["DKI Jakarta", 0.12], ["Aceh", 0.09]],
        "columns": [{"header": "Region"}, {"header": "Rate"}],
    })

    # A worksheet autofilter (not a table): XlsxWriter adds the hidden
    # _xlnm._FilterDatabase name, as Excel does, and hides filtered-out rows.
    fl = wb.add_worksheet("Filtered")
    fl.write_row(0, 0, ["Region", "Status", "Amount"], head)
    status = ["Open", "Paid", "Open", "Invoiced", "Paid", "Open", "Paid", "Open", "Paid"]
    for i, s in enumerate(status, start=1):
        fl.write_row(i, 0, [f"R{i}", s, i * 10])
        if s != "Paid":
            fl.set_row(i, None, None, {"hidden": True})
    fl.autofilter("A1:C10")
    fl.filter_column("B", "x == Paid")
    wb.close()

    feature(f, "table part", "rel:Sales:table", "table[@name=Sales][@ref=A1:D8][@totalsRowCount=1]")
    feature(f, "table style", "rel:Sales:table", "tableStyleInfo[@name=TableStyleMedium9]")
    feature(f, "table autofilter", "rel:Sales:table", "table/autoFilter[@ref=A1:D7]")
    feature(f, "table totals row", "rel:Sales:table", "tableColumn[@name=Units][@totalsRowFunction=sum]")
    feature(f, "table calculated column", "rel:Sales:table",
            "tableColumn[@name=Amount]/calculatedColumnFormula")
    feature(f, "tableParts", "sheet:Sales", "tableParts/tablePart")
    feature(f, "table without filter", "rel:Lookup:table",
            "table[@name=Regions]/tableStyleInfo[@showColumnStripes=1][@showFirstColumn=1]")
    feature(f, "worksheet autofilter", "sheet:Filtered", "autoFilter[@ref=A1:C10]/filterColumn[@colId=1]")
    feature(f, "hidden _FilterDatabase name", "workbook",
            "definedName[@name=_xlnm._FilterDatabase][@hidden=1][@localSheetId=2]")
    feature(f, "filtered rows hidden", "sheet:Filtered", "row[@r=2][@hidden=1]")


# ---------------------------------------------------------------- 14 charts

def f14_charts(path: str) -> None:
    f = "14_charts_images.xlsx"
    wb = new_book(path)
    ws = wb.add_worksheet("Data")
    ws.write_row(0, 0, ["Month", "Plan", "Actual"])
    for i, (plan, actual) in enumerate([(10, 12), (14, 11), (18, 19), (20, 24), (22, 21), (25, 30)], 1):
        ws.write_row(i, 0, [f"M{i}", plan, actual])
    col = wb.add_chart({"type": "column"})
    col.add_series({"name": "=Data!$B$1", "categories": "=Data!$A$2:$A$7", "values": "=Data!$B$2:$B$7"})
    col.add_series({"name": "=Data!$C$1", "categories": "=Data!$A$2:$A$7", "values": "=Data!$C$2:$C$7",
                    "trendline": {"type": "linear"}})
    col.set_title({"name": "Plan vs actual"})
    ws.insert_chart("E2", col)
    ws.insert_image("E18", "swatch.png", {"image_data": io.BytesIO(png(24, 16, (48, 84, 150))),
                                          "description": "Blue swatch"})
    ws.insert_textbox("H18", "A text box", {"width": 160, "height": 40})
    ws.insert_button("E22", {"caption": "Press", "macro": "Noop", "width": 80, "height": 24})

    cs = wb.add_chartsheet("Trend")
    line = wb.add_chart({"type": "line"})
    line.add_series({"name": "Actual", "categories": "=Data!$A$2:$A$7", "values": "=Data!$C$2:$C$7",
                     "marker": {"type": "circle"}})
    cs.set_chart(line)
    # A sheet after the chart sheet: sheet ids and positions must still line up.
    after = wb.add_worksheet("After")
    after.write("A1", "after the chart sheet")
    after.write_formula("A2", "=SUM(Data!C2:C7)", None, 117)
    wb.close()

    feature(f, "chart on a sheet", "sheet:Data", "drawing")
    feature(f, "chart part", "pkg:xl/charts/chart1.xml", "barChart//f[text()=Data!$B$2:$B$7]")
    feature(f, "chart trendline", "pkg:xl/charts/chart1.xml", "trendline/trendlineType[@val=linear]")
    feature(f, "image", "pkg:xl/media/image1.png", "")
    feature(f, "image anchor", "rel:Data:drawing", "pic/nvPicPr/cNvPr[@descr=Blue swatch]")
    feature(f, "text box", "rel:Data:drawing", "sp//t[text()=A text box]")
    feature(f, "form control (VML button)", "rel:Data:vmlDrawing", "")
    feature(f, "form control anchor", "sheet:Data", "legacyDrawing")
    feature(f, "chart sheet in workbook", "workbook", "sheet[@name=Trend][@sheetId=2]")
    feature(f, "chart sheet part", "sheet:Trend", "chartsheet/drawing")
    feature(f, "chart sheet chart", "pkg:xl/charts/chart2.xml", "lineChart//f[text()=Data!$C$2:$C$7]")
    feature(f, "sheet after chart sheet", "sheet:After", "sheetData/row/c[@r=A2]/f")


# ---------------------------------------------------------------- 15 sparklines, CF, DV

def f15_sparklines(path: str) -> None:
    f = "15_sparklines_cf_dv.xlsx"
    wb = new_book(path)
    ws = wb.add_worksheet("Metrics")
    red = wb.add_format({"font_color": "#9C0006", "bg_color": "#FFC7CE"})
    ws.write_row(0, 0, ["Item", "Q1", "Q2", "Q3", "Q4", "Trend"])
    for r in range(1, 9):
        ws.write_string(r, 0, f"Item {r}")
        for c in range(1, 5):
            ws.write_number(r, c, (r * 7 + c * 13) % 50 - 10)
    ws.add_sparkline("F2", {"range": "Metrics!B2:E2"})
    ws.add_sparkline("F3", {"range": "Metrics!B3:E3", "type": "column", "high_point": True})
    ws.add_sparkline("F4", {"range": "Metrics!B4:E4", "type": "win_loss", "negative_points": True})
    ws.add_sparkline("F5", {"range": "Metrics!B5:E5", "markers": True, "series_color": "#C00000"})

    ws.conditional_format("B2:B9", {"type": "cell", "criteria": ">", "value": 30, "format": red})
    ws.conditional_format("C2:C9", {"type": "3_color_scale"})
    ws.conditional_format("D2:D9", {"type": "data_bar", "data_bar_2010": True, "bar_solid": True,
                                    "bar_negative_color": "#FF0000"})
    ws.conditional_format("E2:E9", {"type": "icon_set", "icon_style": "3_arrows"})
    ws.conditional_format("A2:A9", {"type": "formula", "criteria": "=$B2<0", "format": red})
    ws.conditional_format("B2:E9", {"type": "top", "value": 3, "format": red})

    ws.write("H1", "Status")
    ws.data_validation("H2:H9", {"validate": "list", "source": ["Open", "Closed", "Hold"]})
    ws.write_column("K1", ["North", "South", "East"])
    ws.data_validation("I2:I9", {"validate": "list", "source": "=$K$1:$K$3"})
    ws.data_validation("J2:J9", {"validate": "integer", "criteria": "between", "minimum": 1, "maximum": 10,
                                 "input_title": "Score", "input_message": "1 to 10",
                                 "error_title": "Out of range", "error_message": "Use 1 to 10"})
    other = wb.add_worksheet("Lists")
    other.write_column("A1", ["Low", "Medium", "High"])
    ws.data_validation("L2:L9", {"validate": "list", "source": "=Lists!$A$1:$A$3"})
    wb.close()

    x14_spark = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}"
    x14_cf = "{78C0D931-6437-407d-A8EE-F0AAD7539E65}"
    feature(f, "sparklines", "sheet:Metrics", f"extLst/ext[@uri={x14_spark}]//sparkline")
    feature(f, "sparkline group settings", "sheet:Metrics", "sparklineGroup[@type=column][@high=1]")
    feature(f, "sparkline data range", "sheet:Metrics", "sparkline/f[text()=Metrics!B2:E2]")
    feature(f, "x14 data bar", "sheet:Metrics", f"extLst/ext[@uri={x14_cf}]//cfRule[@type=dataBar]")
    feature(f, "cf cell rule", "sheet:Metrics", "conditionalFormatting[@sqref=B2:B9]/cfRule[@type=cellIs]")
    feature(f, "cf color scale", "sheet:Metrics", "conditionalFormatting[@sqref=C2:C9]/cfRule/colorScale")
    feature(f, "cf data bar", "sheet:Metrics", "conditionalFormatting[@sqref=D2:D9]/cfRule/dataBar")
    feature(f, "cf icon set", "sheet:Metrics", "cfRule/iconSet[@iconSet=3Arrows]")
    feature(f, "cf formula", "sheet:Metrics", "conditionalFormatting[@sqref=A2:A9]/cfRule[@type=expression]")
    feature(f, "cf top 10", "sheet:Metrics", "cfRule[@type=top10][@rank=3]")
    feature(f, "validation list (inline)", "sheet:Metrics",
            "dataValidation[@type=list][@sqref=H2:H9]/formula1[text()=\"Open,Closed,Hold\"]")
    feature(f, "validation list (range)", "sheet:Metrics", "dataValidation[@sqref=I2:I9]/formula1[text()=$K$1:$K$3]")
    feature(f, "validation messages", "sheet:Metrics",
            "dataValidation[@type=whole][@promptTitle=Score][@errorTitle=Out of range]")
    feature(f, "validation list (other sheet)", "sheet:Metrics",
            "dataValidation[@sqref=L2:L9]/formula1[text()=Lists!$A$1:$A$3]")


# ---------------------------------------------------------------- 16 print, protection, outline

def f16_print(path: str) -> None:
    f = "16_print_protect_outline.xlsx"
    wb = new_book(path)
    ws = wb.add_worksheet("Print")
    for r in range(40):
        ws.write_row(r, 0, [f"Row {r + 1}", r, r * 2, r * 3])
    ws.set_landscape()
    ws.set_paper(9)
    ws.set_margins(left=0.5, right=0.5, top=0.8, bottom=0.8)
    ws.set_header("&LKantor&CQuarterly report&R&D")
    ws.set_footer("&CPage &P of &N")
    ws.repeat_rows(0)
    ws.print_area("A1:D40")
    ws.fit_to_pages(1, 0)
    ws.center_horizontally()
    ws.hide_gridlines(0)
    ws.set_h_pagebreaks([20])
    ws.write_comment("B2", "Checked by finance", {"author": "Ann"})
    ws.write_comment("C3", "Always shown", {"visible": True, "author": "Budi"})
    ws.write_url("F1", "https://example.com/report", string="Report site")
    ws.write_url("F2", "internal:Outline!A1", string="Go to Outline")
    ws.write_url("F3", "mailto:finance@example.com", string="Mail finance")

    pr = wb.add_worksheet("Protected")
    unlocked = wb.add_format({"locked": False, "bg_color": "#FFF2CC"})
    hidden = wb.add_format({"hidden": True})
    pr.write("A1", "Inputs are yellow")
    pr.write("B2", 5, unlocked)
    pr.write_formula("B3", "=B2*2", hidden, 10)
    pr.protect("", {"format_columns": True, "sort": True, "select_locked_cells": True,
                    "select_unlocked_cells": True})

    ol = wb.add_worksheet("Outline")
    ol.write_row(0, 0, ["Region", "Q1", "Q2", "Q3", "Q4", "Year"])
    for r in range(1, 10):
        ol.write_row(r, 0, [f"Region {r}", r, r + 1, r + 2, r + 3])
        ol.set_row(r, None, None, {"level": 1 if r % 3 else 2, "hidden": r > 6})
    ol.set_row(10, None, None, {"collapsed": True})
    ol.set_column("B:E", 10, None, {"level": 1})
    ol.set_column("G:G", None, None, {"hidden": True})
    ol.write("G1", "hidden column")

    wb.define_name("Rate", "=0.11")
    wb.define_name("Outline!Regions", "=Outline!$A$2:$A$10")
    wb.define_name("Totals", "=Print!$D$1:$D$40")
    wb.close()

    feature(f, "page setup", "sheet:Print", "pageSetup[@orientation=landscape][@paperSize=9][@fitToHeight=0]")
    feature(f, "fit to page flag", "sheet:Print", "sheetPr/pageSetUpPr[@fitToPage=1]")
    feature(f, "page margins", "sheet:Print", "pageMargins[@left=0.5][@top=0.8]")
    feature(f, "header", "sheet:Print", "headerFooter/oddHeader[text()=&LKantor&CQuarterly report&R&D]")
    feature(f, "footer", "sheet:Print", "headerFooter/oddFooter[text()=&CPage &P of &N]")
    feature(f, "print options", "sheet:Print", "printOptions[@horizontalCentered=1]")
    feature(f, "page break", "sheet:Print", "rowBreaks/brk[@id=20]")
    feature(f, "print area name", "workbook", "definedName[@name=_xlnm.Print_Area][@localSheetId=0]")
    feature(f, "print titles name", "workbook", "definedName[@name=_xlnm.Print_Titles][@localSheetId=0]")
    feature(f, "comment", "rel:Print:comments", "comment[@ref=B2]//t[text()=Checked by finance]")
    feature(f, "comment author", "rel:Print:comments", "authors/author[text()=Budi]")
    feature(f, "comment boxes (VML)", "rel:Print:vmlDrawing", "")
    feature(f, "external hyperlink", "sheet:Print", "hyperlinks/hyperlink[@ref=F1][@id]")
    feature(f, "internal hyperlink", "sheet:Print", "hyperlink[@ref=F2][@location=Outline!A1]")
    feature(f, "mailto hyperlink", "sheet:Print", "hyperlink[@ref=F3][@id]")
    feature(f, "sheet protection", "sheet:Protected", "sheetProtection[@sheet=1][@formatColumns=0][@sort=0]")
    feature(f, "unlocked cell", "styles", "cellXfs/xf[@applyProtection=1]/protection[@locked=0]")
    feature(f, "hidden formula", "styles", "cellXfs/xf/protection[@hidden=1]")
    feature(f, "row outline level", "sheet:Outline", "row[@r=2][@outlineLevel=1]")
    feature(f, "row outline level 2", "sheet:Outline", "row[@r=4][@outlineLevel=2]")
    feature(f, "collapsed row", "sheet:Outline", "row[@r=11][@collapsed=1]")
    feature(f, "hidden rows", "sheet:Outline", "row[@r=8][@hidden=1]")
    feature(f, "column outline level", "sheet:Outline", "cols/col[@min=2][@max=5][@outlineLevel=1]")
    feature(f, "hidden column", "sheet:Outline", "cols/col[@min=7][@hidden=1]")
    feature(f, "outline summary", "sheet:Outline", "sheetFormatPr[@outlineLevelRow=2]")
    feature(f, "workbook name", "workbook", "definedName[@name=Rate][text()=0.11]")
    feature(f, "sheet-local name", "workbook", "definedName[@name=Regions][@localSheetId=2]")


# ---------------------------------------------------------------- 17 cell styles

BORDERS = ["thin", "medium", "dashed", "dotted", "thick", "double", "hair", "mediumDashed",
           "dashDot", "mediumDashDot", "dashDotDot", "mediumDashDotDot", "slantDashDot"]


def f17_cell_styles(path: str) -> None:
    f = "17_cell_styles.xlsx"
    wb = new_book(path)
    ws = wb.add_worksheet("Borders")
    ws.set_column("A:A", 20)
    for i, name in enumerate(BORDERS):
        # XlsxWriter's border index i + 1 is OOXML style `name`.
        fmt = wb.add_format({"border": i + 1, "border_color": "#1F4E79"})
        ws.write(i * 2 + 1, 1, name, fmt)
        feature(f, f"border {name}", "styles", f"border/left[@style={name}]/color[@rgb=FF1F4E79]")
    diag = wb.add_format({"diag_type": 3, "diag_border": 1, "diag_color": "#FF0000"})
    ws.write("D2", "diagonal", diag)
    feature(f, "diagonal border", "styles", "border[@diagonalUp=1][@diagonalDown=1]/diagonal[@style=thin]")

    al = wb.add_worksheet("Alignment")
    al.set_column("A:A", 24)
    al.write("A1", "indent 1", wb.add_format({"indent": 1}))
    al.write("A2", "indent 3 right", wb.add_format({"indent": 3, "align": "right"}))
    al.write("A3", "rotated 45", wb.add_format({"rotation": 45}))
    al.write("A4", "rotated -30", wb.add_format({"rotation": -30}))
    al.write("A5", "rotated 90", wb.add_format({"rotation": 90}))
    al.write("A6", "stacked", wb.add_format({"rotation": 270}))
    al.write("A7", "shrink this long text to fit", wb.add_format({"shrink": True}))
    al.write("A8", "wrap + indent", wb.add_format({"text_wrap": True, "indent": 2, "valign": "top"}))
    wb.close()

    feature(f, "indent", "styles", "xf/alignment[@indent=1]")
    feature(f, "indent with right align", "styles", "xf/alignment[@horizontal=right][@indent=3]")
    feature(f, "text rotation 45", "styles", "xf/alignment[@textRotation=45]")
    feature(f, "text rotation -30", "styles", "xf/alignment[@textRotation=120]")
    feature(f, "text rotation 90", "styles", "xf/alignment[@textRotation=90]")
    feature(f, "stacked text", "styles", "xf/alignment[@textRotation=255]")
    feature(f, "shrink to fit", "styles", "xf/alignment[@shrinkToFit=1]")
    feature(f, "wrap with indent", "styles", "xf/alignment[@vertical=top][@wrapText=1][@indent=2]")



# ---------------------------------------------------------------- 18 conditional formatting

def f18_conditional(path: str) -> None:
    """One column per rule type, rows 2-11; expected results (what Excel shows) are in
    crates/sheets_engine/tests/conditional.rs."""
    f = "18_conditional_formatting.xlsx"
    wb = new_book(path)
    ws = wb.add_worksheet("Rules")
    green = wb.add_format({"font_color": "#006100", "bg_color": "#C6EFCE"})
    red = wb.add_format({"font_color": "#9C0006", "bg_color": "#FFC7CE"})
    yellow = wb.add_format({"font_color": "#9C5700", "bg_color": "#FFEB9C"})
    bold = wb.add_format({"bold": True})
    blue_font = wb.add_format({"font_color": "#0000FF"})
    yellow_fill = wb.add_format({"bg_color": "#FFFF00"})
    two_dp = wb.add_format({"num_format": "0.00"})
    rows = range(1, 11)
    fruit = ["apple", "Banana", "cherry", "date", "apple pie", "grape", "", "Apple", "kiwi", "melon"]
    cols = {
        "A": list(range(1, 11)),
        "B": fruit, "C": fruit, "D": fruit, "E": fruit,
        "F": [1, 2, 2, 3, 4, 4, 4, 5, 6, 7], "G": [1, 2, 2, 3, 4, 4, 4, 5, 6, 7],
        "H": [v * 10 for v in range(1, 11)], "I": [v * 10 for v in range(1, 11)],
        "J": list(range(1, 11)), "K": list(range(1, 11)),
        "L": [1, None, 3, None, 5, " ", 7, None, 9, 10],
        "N": list(range(1, 11)), "O": list(range(1, 11)),
        "P": list(range(0, 100, 10)), "Q": list(range(0, 100, 10)), "R": list(range(0, 100, 10)),
        "S": list(range(-5, 5)), "T": list(range(1, 11)),
        "U": list(range(1, 11)), "V": list(range(1, 11)), "W": list(range(1, 11)),
        "X": list(range(1, 11)), "Y": list(range(1, 11)), "Z": list(range(1, 11)),
        "AA": list(range(1, 11)), "AB": list(range(1, 11)), "AC": list(range(1, 11)),
    }
    for col, values in cols.items():
        ws.write(f"{col}1", col)
        for r, v in zip(rows, values):
            if v is None:
                continue
            ws.write(f"{col}{r + 1}", v)
    ws.write("M1", "M")
    errors = [("=1/0", "#DIV/0!"), ("=1", 1), ("=NA()", "#N/A"), ("=2", 2), ('="a"', "a"),
              ("=SQRT(-1)", "#NUM!"), ("=3", 3), ("=4", 4), ("=#REF!", "#REF!"), ("=5", 5)]
    for r, (formula, value) in zip(rows, errors):
        ws.write_formula(f"M{r + 1}", formula, None, value)
    # Dates for the time period rules are typed in by the test (relative to today).
    ws.write("AD1", "AD")
    cf = ws.conditional_format
    cf("A2:A11", {"type": "cell", "criteria": "greater than", "value": 8, "format": red, "stop_if_true": True})
    cf("A2:A11", {"type": "cell", "criteria": "between", "minimum": 3, "maximum": 5, "format": green})
    cf("A2:A11", {"type": "cell", "criteria": "equal to", "value": 7, "format": bold})
    cf("A2:A11", {"type": "cell", "criteria": "not between", "minimum": 2, "maximum": 9, "format": yellow})
    cf("B2:B11", {"type": "text", "criteria": "containing", "value": "apple", "format": green})
    cf("C2:C11", {"type": "text", "criteria": "not containing", "value": "a", "format": green})
    cf("D2:D11", {"type": "text", "criteria": "begins with", "value": "ch", "format": green})
    cf("E2:E11", {"type": "text", "criteria": "ends with", "value": "on", "format": green})
    cf("F2:F11", {"type": "duplicate", "format": red})
    cf("G2:G11", {"type": "unique", "format": green})
    cf("H2:H11", {"type": "top", "value": 3, "format": green})
    cf("I2:I11", {"type": "bottom", "value": 20, "criteria": "%", "format": red})
    cf("J2:J11", {"type": "average", "criteria": "above", "format": green})
    cf("K2:K11", {"type": "average", "criteria": "below", "format": red})
    cf("L2:L11", {"type": "blanks", "format": yellow})
    cf("M2:M11", {"type": "errors", "format": red})
    cf("N2:N11", {"type": "formula", "criteria": "=MOD(N2,2)=0", "format": green})
    cf("O2:O11", {"type": "formula", "criteria": "=$A2>N2*0+5", "format": green})
    cf("P2:P11", {"type": "2_color_scale", "min_color": "#FFFFFF", "max_color": "#FF0000"})
    cf("Q2:Q11", {"type": "3_color_scale", "min_color": "#F8696B", "mid_type": "percentile",
                  "mid_value": 50, "mid_color": "#FFEB84", "max_color": "#63BE7B"})
    cf("R2:R11", {"type": "2_color_scale", "min_type": "num", "min_value": 20, "min_color": "#000000",
                  "max_type": "num", "max_value": 60, "max_color": "#FFFFFF"})
    cf("S2:S11", {"type": "data_bar", "bar_color": "#638EC6", "bar_solid": True, "data_bar_2010": True,
                  "bar_negative_color": "#FF0000"})
    cf("T2:T11", {"type": "data_bar", "bar_color": "#63C384"})
    cf("U2:U11", {"type": "icon_set", "icon_style": "3_arrows"})
    cf("V2:V11", {"type": "icon_set", "icon_style": "3_traffic_lights"})
    cf("W2:W11", {"type": "icon_set", "icon_style": "5_arrows"})
    cf("X2:X11", {"type": "icon_set", "icon_style": "5_quarters"})
    cf("Y2:Y11", {"type": "icon_set", "icon_style": "3_flags", "reverse_icons": True})
    cf("Z2:Z11", {"type": "icon_set", "icon_style": "3_symbols", "icons_only": True,
                  "icons": [{"criteria": ">=", "type": "number", "value": 8},
                            {"criteria": ">", "type": "number", "value": 4}]})
    cf("AA2:AA11", {"type": "cell", "criteria": ">", "value": 5, "format": red, "stop_if_true": True})
    cf("AA2:AA11", {"type": "cell", "criteria": ">", "value": 0, "format": bold})
    cf("AA2:AA11", {"type": "cell", "criteria": ">", "value": 0, "format": yellow_fill})
    cf("AB2:AB11", {"type": "cell", "criteria": ">", "value": 3, "format": blue_font})
    cf("AB2:AB11", {"type": "cell", "criteria": ">", "value": 0, "format": yellow_fill})
    cf("AC2:AC11", {"type": "cell", "criteria": ">", "value": 5, "format": two_dp})
    cf("AD2:AD11", {"type": "time_period", "criteria": "yesterday", "format": red})
    cf("AD2:AD11", {"type": "time_period", "criteria": "last 7 days", "format": green})

    whole = wb.add_worksheet("Columns")
    for r in range(0, 20):
        whole.write_number(r, 0, r * 10)
        whole.write_string(r, 1, "x" if r % 3 == 0 else "y")
    whole.conditional_format("A1:A1048576", {"type": "cell", "criteria": ">", "value": 150, "format": red})
    whole.conditional_format("A1:B20", {"type": "formula", "criteria": '=$B1="x"', "format": yellow})
    wb.close()

    x14_cf = "{78C0D931-6437-407d-A8EE-F0AAD7539E65}"
    feature(f, "cf cellIs between", "sheet:Rules",
            "conditionalFormatting[@sqref=A2:A11]/cfRule[@type=cellIs][@operator=between]")
    feature(f, "cf stopIfTrue", "sheet:Rules", "cfRule[@type=cellIs][@stopIfTrue=1]")
    feature(f, "cf containsText", "sheet:Rules", "cfRule[@type=containsText][@text=apple]/formula")
    feature(f, "cf notContainsText", "sheet:Rules", "cfRule[@type=notContainsText][@operator=notContains]")
    feature(f, "cf beginsWith", "sheet:Rules", "cfRule[@type=beginsWith][@text=ch]")
    feature(f, "cf endsWith", "sheet:Rules", "cfRule[@type=endsWith][@text=on]")
    feature(f, "cf duplicates", "sheet:Rules", "conditionalFormatting[@sqref=F2:F11]/cfRule[@type=duplicateValues]")
    feature(f, "cf unique", "sheet:Rules", "cfRule[@type=uniqueValues]")
    feature(f, "cf bottom percent", "sheet:Rules", "cfRule[@type=top10][@bottom=1][@percent=1][@rank=20]")
    feature(f, "cf below average", "sheet:Rules", "cfRule[@type=aboveAverage][@aboveAverage=0]")
    feature(f, "cf blanks", "sheet:Rules", "cfRule[@type=containsBlanks]/formula")
    feature(f, "cf errors", "sheet:Rules", "cfRule[@type=containsErrors]")
    feature(f, "cf expression", "sheet:Rules", "cfRule[@type=expression]/formula[text()=MOD(N2,2)=0]")
    feature(f, "cf 3-colour percentile", "sheet:Rules", "colorScale/cfvo[@type=percentile][@val=50]")
    feature(f, "cf colour scale numbers", "sheet:Rules", "colorScale/cfvo[@type=num][@val=20]")
    feature(f, "cf x14 data bar negative", "sheet:Rules",
            f"extLst/ext[@uri={x14_cf}]//dataBar/negativeFillColor[@rgb=FFFF0000]")
    feature(f, "cf icons 5 quarters", "sheet:Rules", "cfRule/iconSet[@iconSet=5Quarters]")
    feature(f, "cf icons reversed", "sheet:Rules", "cfRule/iconSet[@iconSet=3Flags][@reverse=1]")
    feature(f, "cf icons only", "sheet:Rules", "cfRule/iconSet[@iconSet=3Symbols2][@showValue=0]")
    feature(f, "cf time period", "sheet:Rules", "cfRule[@type=timePeriod][@timePeriod=last7Days]")
    feature(f, "cf whole column", "sheet:Columns", "conditionalFormatting[@sqref=A1:A1048576]/cfRule[@type=cellIs]")
    feature(f, "cf dxf fill", "styles", "dxfs/dxf/fill/patternFill/bgColor[@rgb=FFC6EFCE]")
    feature(f, "cf dxf number format", "styles", "dxfs/dxf/numFmt[@formatCode=0.00]")


FILES = [
    ("13_tables.xlsx", f13_tables),
    ("14_charts_images.xlsx", f14_charts),
    ("15_sparklines_cf_dv.xlsx", f15_sparklines),
    ("16_print_protect_outline.xlsx", f16_print),
    ("17_cell_styles.xlsx", f17_cell_styles),
    ("18_conditional_formatting.xlsx", f18_conditional),
]
FEATURES_FILE = "features.tsv"


def generate(out: str) -> list[str]:
    os.makedirs(out, exist_ok=True)
    FEATURES.clear()
    for name, make in FILES:
        make(os.path.join(out, name))
    with open(os.path.join(out, FEATURES_FILE), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("# Generated by scripts/corpus/gen_features_xlsx.py. file\tfeature\tpart\tprobe\n")
        for row in FEATURES:
            fh.write("\t".join(row) + "\n")
    return [n for n, _ in FILES] + [FEATURES_FILE]


def unpacked(path: str) -> dict[str, bytes] | bytes | None:
    if not os.path.exists(path):
        return None
    if path.endswith(".xlsx"):
        with zipfile.ZipFile(path) as z:
            return {n: z.read(n) for n in z.namelist()}
    with open(path, "rb") as fh:
        return fh.read()


def check() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        stale = [n for n in generate(tmp)
                 if unpacked(os.path.join(tmp, n)) != unpacked(os.path.join(DEFAULT_OUT, n))]
    for n in stale:
        print(f"out of date: tests/corpus/xlsx/{n}")
    if stale:
        print(f"run: uv run --with xlsxwriter=={WANT_VERSION} scripts/corpus/gen_features_xlsx.py")
        return 1
    print("feature corpus is up to date")
    return 0


def main() -> None:
    if xlsxwriter.__version__ != WANT_VERSION:
        sys.exit(f"needs XlsxWriter {WANT_VERSION} (output differs between versions), "
                 f"found {xlsxwriter.__version__}")
    if sys.argv[1:] == ["--check"]:
        sys.exit(check())
    out = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    for name in generate(out):
        print(f"wrote {name}")


if __name__ == "__main__":
    main()
