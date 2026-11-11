#!/usr/bin/env python3
"""Write the print / PDF export fixture (CC0, synthetic):
tests/corpus/print/print.xlsx, read by crates/sheets_pdf/tests/export.rs.

Sheets, each with page counts that can be worked out by hand (see the test):

* "Report": A4 portrait, margins 0.5/0.5/1/1 in, 101 rows of 20 pt
  (a header row + 100 orders), 6 columns; row 1 repeated on every page
  (print titles), a manual page break before row 51, header
  `&L&A &C&"Arial,Bold"&14Quarterly Report &R&D` and footer
  `&CPage &P of &N`, gridlines printed, centred horizontally.
* "Wide": Letter landscape, 30 columns of width 10 by 40 rows, fit to one
  page wide (any number tall), printed over then down, row and column
  headings.
* "Styled": A4, every border style, fills, merged cells, wrapped text,
  rotation, indent, centre across selection, text overflowing into empty
  neighbours, a number too wide for its column (####), conditional formats
  (colour scale, data bar, icon set, a highlight rule), an Excel table with
  banded rows, a hidden row and a hidden column.
* "Area": F4 (Folio, 8.5 x 13 in) portrait, print area B3:E40 with column A
  and rows 1-2 as print titles, first page number 7.
* "Legal": US Legal, a couple of cells (used range C5:D6 only).
* "Secret": a hidden sheet (left out when printing the whole workbook).

    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_print_xlsx.py [OUT.xlsx]
"""

import argparse
import datetime as dt
from pathlib import Path

import xlsxwriter

ROOT = Path(__file__).resolve().parents[2]
REGIONS = ["Jawa Timur", "Jawa Barat", "Bali", "DKI Jakarta"]


def report(wb, bold, money, date_fmt):
    ws = wb.add_worksheet("Report")
    ws.set_paper(9)  # A4
    ws.set_portrait()
    ws.set_margins(left=0.5, right=0.5, top=1.0, bottom=1.0)
    ws.set_header('&L&A &C&"Arial,Bold"&14Quarterly Report &R&D', {"margin": 0.4})
    ws.set_footer("&CPage &P of &N", {"margin": 0.4})
    ws.repeat_rows(0)
    ws.set_h_pagebreaks([50])  # a new page starts at row 51
    ws.hide_gridlines(0)  # print gridlines
    ws.center_horizontally()
    ws.set_default_row(20, hide_unused_rows=False)
    for r in range(101):
        ws.set_row(r, 20)
    ws.set_column(0, 0, 8)
    ws.set_column(1, 1, 14)
    ws.set_column(2, 2, 18)
    ws.set_column(3, 5, 12)
    for c, h in enumerate(["Order", "Date", "Region", "Qty", "Price", "Amount"]):
        ws.write(0, c, h, bold)
    start = dt.date(2026, 1, 5)
    for i in range(100):
        r = i + 1
        qty = (i * 7) % 23 + 1
        price = 15000 + (i * 1373) % 90000
        ws.write_number(r, 0, 1000 + i)
        ws.write_datetime(r, 1, dt.datetime.combine(start + dt.timedelta(days=i), dt.time()), date_fmt)
        ws.write_string(r, 2, REGIONS[i % len(REGIONS)])
        ws.write_number(r, 3, qty)
        ws.write_number(r, 4, price, money)
        ws.write_formula(r, 5, f"=D{r + 1}*E{r + 1}", money, qty * price)


def wide(wb):
    ws = wb.add_worksheet("Wide")
    ws.set_paper(1)  # Letter
    ws.set_landscape()
    ws.fit_to_pages(1, 0)
    ws.print_across()
    ws.print_row_col_headers()
    ws.set_column(0, 29, 10)
    for r in range(40):
        for c in range(30):
            ws.write_number(r, c, r * 100 + c)


def styled(wb):
    ws = wb.add_worksheet("Styled")
    ws.set_paper(9)
    ws.set_column(0, 0, 22)
    ws.set_column(1, 7, 11)
    names = ["thin", "medium", "dashed", "dotted", "thick", "double", "hair",
             "medium dashed", "dash dot", "medium dash dot", "dash dot dot",
             "medium dash dot dot", "slant dash dot"]
    for i, name in enumerate(names, start=1):
        f = wb.add_format({"bottom": i, "bottom_color": "#1F4E79" if i % 2 else "#C00000"})
        ws.write(i, 0, f"border {name}", f)
    fill = wb.add_format({"bg_color": "#FFF2CC", "border": 1})
    ws.merge_range("C2:E3", "Merged and centred", wb.add_format(
        {"align": "center", "valign": "vcenter", "bold": True, "bg_color": "#DDEBF7", "border": 2}))
    ws.write("C5", "Wrapped text that needs several lines in its cell",
             wb.add_format({"text_wrap": True, "valign": "top", "border": 1}))
    ws.set_row(4, 48)
    ws.write("D5", "Turned", wb.add_format({"rotation": 45, "border": 1}))
    ws.write("E5", "Up", wb.add_format({"rotation": 90, "border": 1}))
    ws.write("C7", "Indented twice", wb.add_format({"indent": 2}))
    across = wb.add_format({"center_across": True, "italic": True, "font_color": "#7030A0"})
    ws.write("C8", "Centred across C8:E8", across)
    ws.write_blank("D8", None, across)
    ws.write_blank("E8", None, across)
    ws.write("C10", "This text overflows into the empty cells on its right")
    ws.write("C11", "Stops", fill)
    ws.write("D11", "here")
    ws.write_number("F12", 123456789012, wb.add_format({"num_format": "#,##0"}))
    ws.write("C13", "Underlined, struck", wb.add_format({"underline": 1, "font_strikeout": True}))
    ws.write("C14", "Shrink this text to fit", wb.add_format({"shrink": True, "border": 1}))
    ws.write("C15", "Red 14 pt bold", wb.add_format({"bold": True, "font_size": 14, "font_color": "#FF0000"}))
    # Conditional formats on G2:G11.
    for i in range(10):
        ws.write_number(1 + i, 6, (i * 37) % 100)
        ws.write_number(1 + i, 7, (i * 53) % 100)
    ws.conditional_format("G2:G11", {"type": "data_bar", "bar_color": "#638EC6"})
    ws.conditional_format("H2:H11", {"type": "3_color_scale"})
    ws.conditional_format("G2:G11", {"type": "icon_set", "icon_style": "3_traffic_lights"})
    ws.conditional_format("A16:A16", {"type": "cell", "criteria": ">", "value": 5,
                                      "format": wb.add_format({"bg_color": "#C6EFCE", "font_color": "#006100"})})
    ws.write_number("A16", 9)
    # A table with banded rows.
    data = [["North", 10, 2.5], ["South", 20, 3.25], ["East", 30, 4.0], ["West", 40, 5.75]]
    ws.add_table("A20:C24", {"data": data, "style": "Table Style Medium 2",
                             "columns": [{"header": "Region"}, {"header": "Units"}, {"header": "Price"}]})
    # A hidden row and column.
    ws.write("A26", "row 26 is shown")
    ws.write("A27", "HIDDENROW")
    ws.set_row(26, None, None, {"hidden": True})
    ws.write("I2", "HIDDENCOL")
    ws.set_column(8, 8, 10, None, {"hidden": True})
    ws.write("J2", "after hidden")


def area(wb, bold):
    ws = wb.add_worksheet("Area")
    ws.set_paper(14)  # Folio / F4
    ws.print_area("B3:E40")
    ws.repeat_rows(0, 1)
    ws.repeat_columns(0)
    ws.set_start_page(7)
    ws.write("A1", "Title row", bold)
    ws.write("A2", "Code", bold)
    for c in range(1, 6):
        ws.write(1, c, f"Col {c}", bold)
    for r in range(2, 60):
        ws.write(r, 0, f"R{r + 1}")
        for c in range(1, 6):
            ws.write_number(r, c, r * 10 + c)
    ws.write("F3", "OUTSIDEAREA")


def legal(wb):
    ws = wb.add_worksheet("Legal")
    ws.set_paper(5)
    ws.write("C5", "Legal paper")
    ws.write("D6", 42)


def secret(wb):
    ws = wb.add_worksheet("Secret")
    ws.write("A1", "SECRETSHEET")
    ws.hide()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", nargs="?", default=str(ROOT / "tests/corpus/print/print.xlsx"))
    args = ap.parse_args()
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    wb = xlsxwriter.Workbook(args.out)
    wb.set_properties({"title": "Print fixture", "created": dt.datetime(2026, 11, 11)})
    bold = wb.add_format({"bold": True, "bg_color": "#DDEBF7", "border": 1})
    money = wb.add_format({"num_format": "#,##0"})
    date_fmt = wb.add_format({"num_format": "yyyy-mm-dd"})
    report(wb, bold, money, date_fmt)
    wide(wb)
    styled(wb)
    area(wb, bold)
    legal(wb)
    secret(wb)
    wb.close()


if __name__ == "__main__":
    main()
