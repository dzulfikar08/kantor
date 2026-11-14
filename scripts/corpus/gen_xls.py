#!/usr/bin/env python3
"""Generate the legacy-format import fixtures in tests/corpus/xls/ (CC0, made-up data).

    uv run --with openpyxl==3.1.5 --with xlwt==1.3.0 --with pillow==13.0.0 \
        --with olefile==0.47 scripts/corpus/gen_xls.py [LO_APPIMAGE]

Builds .xlsx workbooks with openpyxl, then has LibreOffice (headless) convert
them to Excel 97-2003 (.xls), an Excel 97-2003 template (.xlt) and
OpenDocument (.ods). LibreOffice writes ordinary BIFF8, the way Excel does:
FONT/FORMAT/XF/PALETTE/STYLE, MERGEDCELLS, COLINFO, ROW, WINDOW2/PANE, HLINK,
NOTE/TXO, DV, CF, SETUP and margins.

LO_APPIMAGE defaults to ~/.cache/lo/lo.AppImage; its profile goes to
~/.cache/ktmp/lo-gen-xls (absolute paths: the AppImage changes directory).

Files:
* formatted.{xls,xlt,ods}: sheet "Report" (fonts, fills, a custom palette
  colour, borders of several styles and colours, alignment with wrap, indent
  and rotation, number formats incl. custom ones, merges, column widths, a
  hidden column, row heights, a hidden row, frozen panes, a hyperlink, a
  comment, landscape A4 with margins and fit to 1 page wide, a print area
  and print titles); sheet "Plain" (gridlines off, tab colour, a formula
  using a defined name); sheet "Secret" (hidden). Names: a workbook name
  and a sheet-local one.
* features.{xls,ods}: notes (comments) on two sheets, one with two lines;
  data validation (inline list, list from a range, whole number between with
  input and error messages, decimal, date, text length, custom formula, a
  warning alert); conditional formats (cell value between, greater than,
  equal to a text, a formula rule with a relative reference, a rule
  reading another cell absolutely) with font, fill and border formats.
  features.ods also has a sheet "Scales" (not in the .xls: BIFF8 keeps
  Excel 2007 rules apart): two- and three-colour scales, a data bar, an
  icon set and a rating, text, top-N, above-average, duplicate-value and
  date rules, and a validation on a whole column.
* drawings.xls: pictures and charts. Sheet "Data": a column chart with a
  title and axis titles, a PNG picture and a JPEG one. Sheet "Charts": a
  stacked horizontal bar chart, a line chart, a stacked area chart, a pie,
  a doughnut, a scatter chart and a radar chart (not shown by Kantor: an
  import note). "Overview": a chart sheet (LibreOffice writes chart sheets
  as worksheets holding a chart, so the generator rewrites it as a BIFF
  chart sheet: BOUNDSHEET type 2 and the chart substream in place of the
  worksheet's).
* dates1904.xls: a workbook in the 1904 date system.
* xlwt.xls: written directly as BIFF8 by xlwt (headless LibreOffice drops
  view settings): frozen rows and columns, the second sheet active, pattern
  fills, an unlocked cell, page breaks, header and footer, centring,
  gridlines off on the second sheet.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from openpyxl import Workbook
from openpyxl.comments import Comment
from openpyxl.workbook.defined_name import DefinedName
from openpyxl.styles import Alignment, Border, Font, PatternFill, Side
from openpyxl.utils.datetime import CALENDAR_MAC_1904
import datetime

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests" / "corpus" / "xls"


def solid(color):
    return PatternFill(patternType="solid", fgColor=color)


def formatted(path: Path) -> None:
    wb = Workbook()
    ws = wb.active
    ws.title = "Report"

    for col, width in zip("ABCDEF", [24, 14, 14, 14, 14, 30]):
        ws.column_dimensions[col].width = width
    ws.column_dimensions["G"].hidden = True
    ws["G1"] = "hidden column"

    ws.merge_cells("A1:F1")
    ws["A1"] = "Quarterly sales, East region"
    ws["A1"].font = Font(name="Arial", size=16, bold=True, color="FFFFFF")
    ws["A1"].fill = solid("1F4E78")
    ws["A1"].alignment = Alignment(horizontal="center", vertical="center")
    ws.row_dimensions[1].height = 30

    heads = ["Item", "Q1", "Q2", "Q3", "Q4", "Note"]
    thin = Side(style="thin", color="000000")
    for i, h in enumerate(heads, start=1):
        c = ws.cell(row=2, column=i, value=h)
        c.font = Font(bold=True, color="1F4E78")
        # Not one of the 56 default palette colours: it goes into PALETTE.
        c.fill = solid("DDEBF7")
        c.border = Border(bottom=Side(style="medium", color="1F4E78"))
        c.alignment = Alignment(horizontal="center")

    rows = [
        ("Rice", 1200.5, 1300.25, 1250, 1400, "steady"),
        ("Coffee", 800, 950.75, 1020, 990, "a long note that wraps onto a second line"),
        ("Tea", -150, 300, 320, 410, "loss in Q1"),
    ]
    for r, row in enumerate(rows, start=3):
        for c, v in enumerate(row, start=1):
            cell = ws.cell(row=r, column=c, value=v)
            if 2 <= c <= 5:
                cell.number_format = '#,##0.00;[Red]-#,##0.00'
                cell.border = Border(left=thin, right=thin, top=thin, bottom=thin)
    ws["F4"].alignment = Alignment(wrap_text=True, vertical="top")
    ws.row_dimensions[4].height = 32
    ws["A5"].alignment = Alignment(indent=2)
    ws["A5"].font = Font(italic=True, underline="single", strike=True)

    ws["A6"] = "Total"
    ws["A6"].font = Font(bold=True)
    for c in "BCDE":
        ws[f"{c}6"] = f"=SUM({c}3:{c}5)"
        ws[f"{c}6"].number_format = '"Rp "#,##0'
        ws[f"{c}6"].border = Border(top=Side(style="thin"), bottom=Side(style="double"))
    ws["F6"] = 0.375
    ws["F6"].number_format = "0.0%"

    ws["A8"] = "Rotated"
    ws["A8"].alignment = Alignment(text_rotation=45)
    ws["B8"] = "Vertical"
    ws["B8"].alignment = Alignment(text_rotation=90)
    ws["C8"] = "Right"
    ws["C8"].alignment = Alignment(horizontal="right", vertical="bottom")
    ws["D8"] = datetime.date(2024, 3, 15)
    ws["D8"].number_format = "dd/mm/yyyy"
    ws["E8"] = "dashed"
    ws["E8"].border = Border(
        left=Side(style="dashed", color="FF0000"),
        right=Side(style="dotted", color="00B050"),
        top=Side(style="thick", color="7030A0"),
        bottom=Side(style="hair"),
    )
    ws["F8"] = "Courier 9"
    ws["F8"].font = Font(name="Courier New", size=9, color="C00000")
    ws.row_dimensions[8].height = 48

    ws.merge_cells("A10:B11")
    ws["A10"] = "merged block"
    ws["A10"].alignment = Alignment(horizontal="center", vertical="center")
    ws["A10"].fill = solid("FFFF00")

    ws["A12"] = "hidden row"
    ws.row_dimensions[12].hidden = True

    ws["A14"] = "Kantor site"
    ws["A14"].hyperlink = "https://kantor.paparanai.id/"
    ws["A15"] = "Has a note"
    ws["A15"].comment = Comment("Checked by finance", "Auditor")
    ws["A16"] = 1234.5678
    ws["A16"].number_format = "0.000E+00"
    ws["B16"] = "text"
    ws["B16"].number_format = "@"
    ws["C16"] = True

    ws.freeze_panes = "B3"
    ws.page_setup.orientation = "landscape"
    ws.page_setup.paperSize = ws.PAPERSIZE_A4
    ws.page_setup.fitToWidth = 1
    ws.page_setup.fitToHeight = 0
    ws.sheet_properties.pageSetUpPr.fitToPage = True
    ws.page_margins.left = 0.5
    ws.page_margins.right = 0.5
    ws.page_margins.top = 1.0
    ws.page_margins.bottom = 1.0
    ws.print_area = "A1:F16"
    ws.print_title_rows = "1:2"
    wb.defined_names["Rate"] = DefinedName("Rate", attr_text="Report!$F$6")
    ws.defined_names["Local"] = DefinedName("Local", attr_text="Report!$B$3:$B$5")

    plain = wb.create_sheet("Plain")
    plain.sheet_view.showGridLines = False
    plain.sheet_properties.tabColor = "FF0000"
    plain["A1"] = "No gridlines"
    plain["B2"] = "=Report!B6*2"
    plain["B3"] = "=Rate*100"

    secret = wb.create_sheet("Secret")
    secret.sheet_state = "hidden"
    secret["A1"] = "hidden sheet"
    wb.active = 0
    wb.save(path)


def features(path: Path, scales: bool = False) -> None:
    from openpyxl.formatting.rule import CellIsRule, FormulaRule
    from openpyxl.worksheet.datavalidation import DataValidation

    wb = Workbook()
    ws = wb.active
    ws.title = "Checks"
    heads = ["Item", "Qty", "Price", "Due", "Code", "Status"]
    for i, h in enumerate(heads, start=1):
        ws.cell(row=1, column=i, value=h)
    data = [
        ("Rice", 5, 12.5, datetime.date(2024, 1, 10), "AB12", "Open"),
        ("Coffee", 50, 99.0, datetime.date(2024, 2, 1), "CD34", "Closed"),
        ("Tea", 150, 4.25, datetime.date(2024, 3, 5), "EF56", "Open"),
        ("Sugar", -3, 7.0, datetime.date(2024, 4, 20), "GH78", "Late"),
    ]
    for r, row in enumerate(data, start=2):
        for c, v in enumerate(row, start=1):
            ws.cell(row=r, column=c, value=v)
    ws["H1"] = "Open"
    ws["H2"] = "Closed"
    ws["H3"] = "Late"
    ws["I1"] = 100

    ws["A2"].comment = Comment("Bought weekly", "Ann")
    ws["C3"].comment = Comment("Price went up\nin February", "Budi")

    def dv(**kw):
        sqref = kw.pop("sqref")
        v = DataValidation(**kw)
        v.add(sqref)
        ws.add_data_validation(v)

    dv(type="list", formula1='"Open,Closed,Late"', sqref="F2:F5")
    dv(type="list", formula1="$H$1:$H$3", sqref="G2:G5")
    dv(
        type="whole",
        operator="between",
        formula1="0",
        formula2="100",
        sqref="B2:B5",
        showInputMessage=True,
        promptTitle="Quantity",
        prompt="A whole number from 0 to 100",
        showErrorMessage=True,
        errorTitle="Too many",
        error="Use 0 to 100",
    )
    dv(
        type="decimal",
        operator="greaterThan",
        formula1="0",
        sqref="C2:C5",
        showErrorMessage=True,
        errorStyle="warning",
    )
    dv(
        type="date",
        operator="between",
        formula1="DATE(2024,1,1)",
        formula2="DATE(2024,12,31)",
        sqref="D2:D5",
        showErrorMessage=True,
    )
    dv(type="textLength", operator="equal", formula1="4", sqref="E2:E5", showErrorMessage=True)
    dv(type="custom", formula1="ISNUMBER(I1)", sqref="I1", showErrorMessage=True)

    red = Font(color="9C0006", bold=True)
    pink = PatternFill(start_color="FFC7CE", end_color="FFC7CE", fill_type="solid")
    green = PatternFill(start_color="C6EFCE", end_color="C6EFCE", fill_type="solid")
    boxed = Border(left=Side(style="thin", color="FF0000"), bottom=Side(style="thin", color="FF0000"))
    ws.conditional_formatting.add(
        "B2:B5", CellIsRule(operator="between", formula=["1", "10"], fill=green)
    )
    ws.conditional_formatting.add(
        "C2:C5", CellIsRule(operator="greaterThan", formula=["$I$1"], font=red, border=boxed)
    )
    ws.conditional_formatting.add(
        "F2:F5", CellIsRule(operator="equal", formula=['"Late"'], fill=pink, font=Font(italic=True))
    )
    ws.conditional_formatting.add(
        "A2:A5", FormulaRule(formula=["AND($B2<0,LEN(A2)>3)"], font=Font(color="0000FF", underline="single"))
    )

    other = wb.create_sheet("Other")
    other["B2"] = "noted"
    other["B2"].comment = Comment("On the second sheet", "Ann")
    if scales:
        scales_sheet(wb)
    wb.active = 0
    wb.save(path)


def scales_sheet(wb) -> None:
    from openpyxl.formatting.rule import (
        ColorScaleRule,
        DataBarRule,
        IconSetRule,
        Rule,
    )
    from openpyxl.styles.differential import DifferentialStyle
    from openpyxl.worksheet.datavalidation import DataValidation

    ws = wb.create_sheet("Scales")
    words = ["apple", "banana", "apricot", "cherry", "apple", "date", "fig", "grape", "kiwi", "lime"]
    for i in range(10):
        r = i + 1
        for col in "ABCDE":
            ws[f"{col}{r}"] = r
        ws[f"F{r}"] = words[i]
        ws[f"G{r}"] = r * 10
        ws[f"H{r}"] = r
    ws["I1"] = datetime.date(2024, 1, 1)
    cf = ws.conditional_formatting
    cf.add("A1:A10", ColorScaleRule(start_type="min", start_color="F8696B",
                                    end_type="max", end_color="63BE7B"))
    cf.add("B1:B10", ColorScaleRule(start_type="num", start_value=2, start_color="F8696B",
                                    mid_type="percentile", mid_value=50, mid_color="FFEB84",
                                    end_type="percent", end_value=90, end_color="63BE7B"))
    cf.add("C1:C10", DataBarRule(start_type="min", end_type="max", color="638EC6"))
    cf.add("D1:D10", IconSetRule("3Arrows", "percent", [0, 33, 67]))
    cf.add("E1:E10", IconSetRule("5Rating", "percent", [0, 20, 40, 60, 80], showValue=False))
    yellow = DifferentialStyle(fill=PatternFill(bgColor="FFEB9C", fill_type="solid"))
    text = Rule(type="containsText", operator="containsText", text="ap", dxf=yellow)
    text.formula = ['NOT(ISERROR(SEARCH("ap",F1)))']
    cf.add("F1:F10", text)
    begins = Rule(type="beginsWith", operator="beginsWith", text="b",
                  dxf=DifferentialStyle(font=Font(bold=True)))
    begins.formula = ['LEFT(F1,LEN("b"))="b"']
    cf.add("F1:F10", begins)
    cf.add("F1:F10", Rule(type="duplicateValues",
                          dxf=DifferentialStyle(font=Font(color="9C0006"))))
    cf.add("G1:G10", Rule(type="top10", rank=3,
                          dxf=DifferentialStyle(font=Font(italic=True))))
    cf.add("H1:H10", Rule(type="aboveAverage",
                          dxf=DifferentialStyle(font=Font(color="0000FF"))))
    today = Rule(type="timePeriod", timePeriod="today",
                 dxf=DifferentialStyle(font=Font(strike=True)))
    today.formula = ["FLOOR(I1,1)=TODAY()"]
    cf.add("I1:I3", today)
    dv = DataValidation(type="whole", operator="greaterThanOrEqual", formula1="1",
                        allow_blank=True, showErrorMessage=True, errorStyle="information",
                        errorTitle="Check", error="1 or more")
    dv.add("H1:H1048576")
    ws.add_data_validation(dv)


def picture(fmt: str, size: tuple[int, int], color: str):
    import io

    from PIL import Image as PImage, ImageDraw

    im = PImage.new("RGB", size, "white")
    d = ImageDraw.Draw(im)
    d.rectangle([4, 4, size[0] - 5, size[1] - 5], outline=color, width=4)
    d.ellipse([size[0] // 4, size[1] // 4, size[0] * 3 // 4, size[1] * 3 // 4], fill=color)
    out = io.BytesIO()
    im.save(out, fmt)
    out.seek(0)
    return out


def drawings(path: Path) -> None:
    from openpyxl.chart import (AreaChart, BarChart, DoughnutChart, LineChart, PieChart,
                                RadarChart, Reference, ScatterChart, Series)
    from openpyxl.drawing.image import Image

    wb = Workbook()
    ws = wb.active
    ws.title = "Data"
    ws.append(["Quarter", "North", "South"])
    for row in [("Q1", 10, 7), ("Q2", 14, 9), ("Q3", 12, 15), ("Q4", 18, 11)]:
        ws.append(row)
    data = Reference(ws, min_col=2, min_row=1, max_col=3, max_row=5)
    cats = Reference(ws, min_col=1, min_row=2, max_row=5)

    def chart(c, title, refs=data):
        c.title = title
        c.add_data(refs, titles_from_data=True)
        c.set_categories(cats)
        return c

    col = chart(BarChart(), "Sales by quarter")
    col.type = "col"
    col.x_axis.title = "Quarter"
    col.y_axis.title = "Units"
    ws.add_chart(col, "E2")
    ws.add_image(Image(picture("PNG", (120, 80), "#1F77B4")), "A8")
    ws.add_image(Image(picture("JPEG", (90, 60), "#D62728")), "C8")

    sheet = wb.create_sheet("Charts")
    bar = chart(BarChart(), "Stacked bars")
    bar.type = "bar"
    bar.grouping = "stacked"
    bar.overlap = 100
    sheet.add_chart(bar, "A1")
    sheet.add_chart(chart(LineChart(), "Line"), "J1")
    area = chart(AreaChart(), "Area")
    area.grouping = "stacked"
    sheet.add_chart(area, "A17")
    sheet.add_chart(chart(PieChart(), "Pie", Reference(ws, min_col=2, min_row=1, max_row=5)), "J17")
    sheet.add_chart(chart(DoughnutChart(), "Doughnut",
                          Reference(ws, min_col=3, min_row=1, max_row=5)), "A33")
    scatter = ScatterChart()
    scatter.title = "Scatter"
    scatter.series.append(Series(Reference(ws, min_col=3, min_row=2, max_row=5),
                                 Reference(ws, min_col=2, min_row=2, max_row=5),
                                 title="South vs North"))
    sheet.add_chart(scatter, "J33")
    sheet.add_chart(chart(RadarChart(), "Radar"), "A49")

    overview = wb.create_chartsheet("Overview")
    overview.add_chart(chart(BarChart(), "Overview"))
    wb.save(path)


def make_chart_sheet(path: Path, name: str) -> None:
    """Turn worksheet `name` (holding one chart) into a BIFF8 chart sheet:
    its BOUNDSHEET type becomes 2 and its substream the chart's (the rest
    of the old substream is zero-filled: the stream keeps its size)."""
    import struct

    import olefile

    ole = olefile.OleFileIO(str(path), write_mode=True)
    data = bytearray(ole.openstream("Workbook").read())

    def records(pos):
        while pos + 4 <= len(data):
            typ, size = struct.unpack_from("<HH", data, pos)
            yield pos, typ, bytes(data[pos + 4:pos + 4 + size])
            pos += 4 + size

    offset = None
    for pos, typ, body in records(0):
        if typ == 0x0085 and body[8:].decode("latin-1").endswith(name):
            offset = struct.unpack_from("<I", body)[0]
            data[pos + 4 + 5] = 2
        if typ == 0x000A:
            break
    if offset is None:
        sys.exit(f"no sheet {name}")
    depth, start, end = 0, None, None
    for pos, typ, body in records(offset):
        if typ == 0x0809:
            depth += 1
            if depth == 2:
                start = pos
        elif typ == 0x000A:
            depth -= 1
            if depth == 1 and start is not None and end is None:
                end = pos + 4
            if depth == 0:
                sheet_end = pos + 4
                break
    if start is None:
        sys.exit(f"no chart on {name}")
    chart = bytes(data[start:end])
    data[offset:sheet_end] = chart + bytes(sheet_end - offset - len(chart))
    ole.write_stream("Workbook", bytes(data))
    ole.close()


def dates1904(path: Path) -> None:
    wb = Workbook()
    wb.epoch = CALENDAR_MAC_1904
    ws = wb.active
    ws.title = "Mac"
    ws["A1"] = datetime.date(2024, 3, 15)
    ws["A1"].number_format = "yyyy-mm-dd"
    ws["A2"] = 2.5
    wb.save(path)


def xlwt_book(path: Path) -> None:
    import xlwt

    wb = xlwt.Workbook()
    ws = wb.add_sheet("Frozen")
    head = xlwt.easyxf(
        "font: bold on, colour white; pattern: pattern solid, fore_colour dark_blue;"
        "align: horiz center; borders: bottom medium"
    )
    grey = xlwt.easyxf("pattern: pattern sparse_dots, fore_colour black, back_colour white")
    unlocked = xlwt.easyxf("protection: cell_locked false")
    for c, h in enumerate(["Name", "Jan", "Feb", "Mar"]):
        ws.write(0, c, h, head)
    for r in range(1, 40):
        ws.write(r, 0, f"Row {r}")
        for c in range(1, 4):
            ws.write(r, c, r * c)
    ws.write(1, 4, "25% grey", grey)
    ws.write(2, 4, "unlocked", unlocked)
    ws.col(0).width = 20 * 256
    ws.set_panes_frozen(True)
    ws.set_horz_split_pos(1)
    ws.set_vert_split_pos(1)
    ws.horz_page_breaks = [(20, 0, 255)]
    ws.header_str = b"&CQuarterly"
    ws.footer_str = b"&RPage &P"
    ws.print_centered_horz = True
    ws.portrait = False
    ws.paper_size_code = 9  # A4

    other = wb.add_sheet("Second")
    other.write(0, 0, "opens here")
    other.show_grid = False
    other.set_selected(True)
    wb.set_active_sheet(1)
    ws.set_selected(False)
    wb.save(str(path))


def convert(lo: Path, src: Path, filt: str, ext: str, outdir: Path) -> Path:
    profile = Path.home() / ".cache" / "ktmp" / "lo-gen-xls"
    cmd = [
        str(lo),
        f"-env:UserInstallation=file://{profile}",
        "--headless",
        "--convert-to",
        f"{ext}:{filt}",
        "--outdir",
        str(outdir),
        str(src),
    ]
    # 81: LibreOffice made a new profile and wants a restart.
    for _ in range(3):
        if subprocess.run(cmd, stdout=subprocess.DEVNULL).returncode != 81:
            break
    out = outdir / (src.stem + "." + ext)
    if not out.exists():
        sys.exit(f"LibreOffice did not write {out}")
    return out


def main() -> None:
    lo = Path(sys.argv[1] if len(sys.argv) > 1 else Path.home() / ".cache/lo/lo.AppImage")
    OUT.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(dir=Path.home() / ".cache" / "ktmp"))
    try:
        src = tmp / "formatted.xlsx"
        formatted(src)
        for filt, ext in [
            ("MS Excel 97", "xls"),
            ("MS Excel 97 Vorlage/Template", "xlt"),
            ("calc8", "ods"),
        ]:
            shutil.copy(convert(lo, src, filt, ext, tmp), OUT / f"formatted.{ext}")
        src = tmp / "features.xlsx"
        features(src)
        shutil.copy(convert(lo, src, "MS Excel 97", "xls", tmp), OUT / "features.xls")
        src = tmp / "ods" / "features.xlsx"
        src.parent.mkdir()
        features(src, scales=True)
        shutil.copy(convert(lo, src, "calc8", "ods", src.parent), OUT / "features.ods")
        src = tmp / "drawings.xlsx"
        drawings(src)
        out = convert(lo, src, "MS Excel 97", "xls", tmp)
        make_chart_sheet(out, "Overview")
        shutil.copy(out, OUT / "drawings.xls")
        src = tmp / "dates1904.xlsx"
        dates1904(src)
        shutil.copy(convert(lo, src, "MS Excel 97", "xls", tmp), OUT / "dates1904.xls")
        xlwt_book(OUT / "xlwt.xls")
    finally:
        shutil.rmtree(tmp)


if __name__ == "__main__":
    os.environ.setdefault("SAL_USE_VCLPLUGIN", "svp")
    main()
