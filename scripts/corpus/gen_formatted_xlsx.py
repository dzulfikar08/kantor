#!/usr/bin/env python3
"""Generate a small, heavily formatted .xlsx for checking how Sheets draws
formatting (CC0, no real data).

    uv run --with openpyxl scripts/corpus/gen_formatted_xlsx.py [OUT.xlsx]

Sheet "Report": a merged title row, a big coloured header font, header fills
(RGB, theme colours with tints, an indexed colour), coloured fonts, thin /
medium / thick / double / dashed borders, an outlined total row, wrapped
text, vertical alignment and a merged block spanning rows. Sheet "Plain":
gridlines off, white fill, an Arial cell.
"""

import sys

from openpyxl import Workbook
from openpyxl.styles import Alignment, Border, Color, Font, PatternFill, Side


def solid(color):
    return PatternFill(patternType="solid", fgColor=color)


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "formatted.xlsx"
    wb = Workbook()
    ws = wb.active
    ws.title = "Report"
    for col, width in zip("ABCDEF", [22, 14, 14, 14, 14, 30]):
        ws.column_dimensions[col].width = width

    ws.merge_cells("A1:F1")
    ws["A1"] = "Quarterly sales — East region"
    ws["A1"].font = Font(name="Calibri", size=16, bold=True, color="FFFFFF")
    ws["A1"].fill = solid("1F4E78")
    ws["A1"].alignment = Alignment(horizontal="center", vertical="center")

    ws.merge_cells("A2:F2")
    ws["A2"] = "Figures in million rupiah; draft for review"
    ws["A2"].font = Font(italic=True, color=Color(theme=1, tint=0.5))
    ws["A2"].alignment = Alignment(horizontal="center")

    thin = Side(style="thin", color="808080")
    medium = Side(style="medium", color="1F4E78")
    headers = ["Product", "Q1", "Q2", "Q3", "Q4", "Notes"]
    # Theme accents with tints, plus one indexed colour (44 = light blue).
    header_fills = [
        solid("DDEBF7"),
        solid(Color(theme=4, tint=0.6)),
        solid(Color(theme=5, tint=0.6)),
        solid(Color(theme=9, tint=0.4)),
        solid(Color(indexed=44)),
        solid(Color(theme=7, tint=0.8)),
    ]
    for i, (h, fill) in enumerate(zip(headers, header_fills), start=1):
        c = ws.cell(row=3, column=i, value=h)
        c.font = Font(bold=True, color="1F4E78")
        c.fill = fill
        c.border = Border(top=medium, bottom=medium, left=thin, right=thin)
        c.alignment = Alignment(horizontal="center", vertical="center")

    rows = [
        ("Centrifugal pumps", 120, 135, 128, 150,
         "Steady growth; the Q4 jump comes from two plant orders."),
        ("Gate valves", 80, 72, 91, 88, "Short"),
        ("Bearings", 45, 52, 49, 61,
         "Supplier change in Q2 lowered unit cost by about four percent."),
        ("Motors", 210, 190, 205, 230, ""),
    ]
    for r, row in enumerate(rows, start=4):
        for c, v in enumerate(row, start=1):
            cell = ws.cell(row=r, column=c, value=v)
            cell.border = Border(left=thin, right=thin, top=thin, bottom=thin)
            if c in (2, 3, 4, 5):
                cell.number_format = "#,##0"
            if c == 6:
                cell.alignment = Alignment(wrap_text=True, vertical="top")
        if r % 2 == 1:
            for c in range(1, 6):
                ws.cell(row=r, column=c).fill = solid("F2F2F2")
    ws["B6"].font = Font(color="C00000", bold=True)
    ws["C5"].font = Font(color="00B050")

    total = 8
    ws.cell(row=total, column=1, value="Total").font = Font(bold=True)
    for c, col in enumerate("BCDE", start=2):
        cell = ws.cell(row=total, column=c, value=f"=SUM({col}4:{col}7)")
        cell.number_format = "#,##0"
        cell.font = Font(bold=True)
    double = Side(style="double", color="000000")
    thick = Side(style="thick", color="C00000")
    for c in range(1, 7):
        ws.cell(row=total, column=c).border = Border(
            top=thin, bottom=double, left=thick if c == 1 else None,
            right=thick if c == 6 else None)

    ws["A10"] = "Dashed box"
    dashed = Side(style="dashed", color="7030A0")
    ws["A10"].border = Border(left=dashed, right=dashed, top=dashed, bottom=dashed)
    ws["C10"] = "Top"
    ws["C10"].alignment = Alignment(vertical="top")
    ws["D10"] = "Middle"
    ws["D10"].alignment = Alignment(vertical="center")
    ws["E10"] = "Bottom"
    ws.row_dimensions[10].height = 30

    ws.merge_cells("A12:B14")
    ws["A12"] = "Merged block over three rows, wrapped and centred"
    ws["A12"].alignment = Alignment(wrap_text=True, horizontal="center", vertical="center")
    ws["A12"].fill = solid("FFF2CC")
    box = Side(style="medium", color="BF8F00")
    for r in range(12, 15):
        for c in range(1, 3):
            ws.cell(row=r, column=c).border = Border(
                top=box if r == 12 else None, bottom=box if r == 14 else None,
                left=box if c == 1 else None, right=box if c == 2 else None)
    ws["D12"] = "Big"
    ws["D12"].font = Font(size=20, bold=True, color="2F5597")
    ws["E12"] = "Small"
    ws["E12"].font = Font(size=8, color="7F7F7F")
    ws["D13"] = "Serif"
    ws["D13"].font = Font(name="Times New Roman", size=12)

    plain = wb.create_sheet("Plain")
    plain.sheet_view.showGridLines = False
    plain["B2"] = "No gridlines on this sheet"
    plain["B2"].font = Font(bold=True, size=14)
    plain["B4"] = "White fill"
    plain["B4"].fill = solid("FFFFFF")
    plain["B5"] = "Arial text"
    plain["B5"].font = Font(name="Arial")

    wb.save(out)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
