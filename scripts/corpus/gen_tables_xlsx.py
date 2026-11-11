#!/usr/bin/env python3
"""Write the small Excel-table / sort / filter fixture (CC0, synthetic):
crates/sheets_engine/tests/fixtures/tables.xlsx, read by
crates/sheets_engine/tests/tables.rs.

* "Sales": an Excel table `Sales` (A1:E7, TableStyleMedium2, banded rows,
  filter buttons, a total row with SUBTOTAL) with a calculated column
  `Total = Sales[[#This Row],[Units]]*Sales[[#This Row],[Unit Price]]`, and
  formulas next to it using structured references.
  A second table `Rates` (I1:K4, no total row, a calculated column).
* "Plain": a plain range with a sheet AutoFilter (A1:C9) filtering Region to
  North and East, the other rows hidden as Excel saves them, a date column.
* "Sort": rows to sort: mixed types, blanks, duplicates (stability), formulas
  referring to their own row and an absolute reference.

    uv run --with openpyxl scripts/corpus/gen_tables_xlsx.py [OUT.xlsx]
"""

import argparse
import datetime as dt
from pathlib import Path

from openpyxl import Workbook
from openpyxl.styles import Font
from openpyxl.worksheet.filters import AutoFilter, FilterColumn, Filters
from openpyxl.worksheet.table import Table, TableColumn, TableFormula, TableStyleInfo

ROOT = Path(__file__).resolve().parents[2]


def sales(wb: Workbook) -> None:
    ws = wb.active
    ws.title = "Sales"
    head = ["Region", "Rep", "Units", "Unit Price", "Total"]
    rows = [
        ("North", "Ani", 10, 2.5),
        ("South", "Budi", 20, 3),
        ("East", "Citra", 5, 4),
        ("West", "Dewi", 8, 1.5),
        ("North", "Eko", 12, 2),
    ]
    ws.append(head)
    for i, r in enumerate(rows, start=2):
        ws.append(list(r) + [f"=Sales[[#This Row],[Units]]*Sales[[#This Row],[Unit Price]]"])
    ws.append(["Total", None, "=SUBTOTAL(109,Sales[Units])", None, "=SUBTOTAL(109,Sales[Total])"])
    t = Table(displayName="Sales", ref="A1:E7")
    t.totalsRowCount = 1
    t.tableStyleInfo = TableStyleInfo(name="TableStyleMedium2", showRowStripes=True)
    cols = [TableColumn(id=i + 1, name=h) for i, h in enumerate(head)]
    cols[0].totalsRowLabel = "Total"
    cols[2].totalsRowFunction = "sum"
    cols[4].totalsRowFunction = "sum"
    cols[4].calculatedColumnFormula = TableFormula(attr_text="Sales[[#This Row],[Units]]*Sales[[#This Row],[Unit Price]]")
    t.tableColumns = cols
    t.autoFilter = AutoFilter(ref="A1:E6")
    ws.add_table(t)
    ws["G1"] = "=SUM(Sales[Units])"
    ws["G2"] = "=Sales[[#Totals],[Units]]"
    ws["G3"] = "=ROWS(Sales[#All])"
    ws["G4"] = "=COUNTA(Sales[#Headers])"
    ws["G5"] = "=SUM(Sales[[Units]:[Unit Price]])"
    ws["G6"] = "=SUBTOTAL(9,Sales[Units])"
    # A second table, without a total row: rows typed under it join it.
    ws["I1"], ws["J1"], ws["K1"] = "Code", "Rate", "Double"
    for i, (code, rate) in enumerate([("A", 1), ("B", 2), ("C", 3)], start=2):
        ws.cell(i, 9, code)
        ws.cell(i, 10, rate)
        ws.cell(i, 11, "=Rates[[#This Row],[Rate]]*2")
    r = Table(displayName="Rates", ref="I1:K4")
    r.tableStyleInfo = TableStyleInfo(name="TableStyleLight9", showRowStripes=True)
    r.tableColumns = [TableColumn(id=i + 1, name=h) for i, h in enumerate(["Code", "Rate", "Double"])]
    r.tableColumns[2].calculatedColumnFormula = TableFormula(attr_text="Rates[[#This Row],[Rate]]*2")
    ws.add_table(r)


def plain(wb: Workbook) -> None:
    ws = wb.create_sheet("Plain")
    ws.append(["Region", "Amount", "Day"])
    data = [
        ("North", 100, dt.date(2025, 1, 3)),
        ("South", 200, dt.date(2025, 1, 15)),
        ("East", 300, dt.date(2025, 2, 1)),
        ("West", 400, dt.date(2025, 2, 20)),
        ("North", 500, dt.date(2025, 3, 5)),
        ("South", 600, dt.date(2025, 3, 9)),
        ("East", 700, dt.date(2025, 4, 1)),
        ("west", 800, dt.date(2025, 4, 2)),
    ]
    for r in data:
        ws.append(list(r))
    for row in range(2, 10):
        ws.cell(row, 3).number_format = "yyyy-mm-dd"
    ws.auto_filter.ref = "A1:C9"
    ws.auto_filter.filterColumn.append(FilterColumn(colId=0, filters=Filters(filter=["North", "East"])))
    for i, r in enumerate(data, start=2):
        if r[0] not in ("North", "East"):
            ws.row_dimensions[i].hidden = True
    ws["E1"] = "=SUBTOTAL(9,B2:B9)"
    ws["E2"] = "=SUBTOTAL(109,B2:B9)"
    ws["E3"] = "=SUM(B2:B9)"


def sort_sheet(wb: Workbook) -> None:
    ws = wb.create_sheet("Sort")
    ws.append(["Key", "Order", "Double", "Times rate"])
    ws["F1"] = 10  # the rate, referred to absolutely
    ws["A1"].font = Font(bold=True)
    keys = ["banana", 3, None, "Apple", True, "cherry", 1, "apple", False, 2.5]
    for i, k in enumerate(keys, start=2):
        ws.cell(i, 1, k)
        ws.cell(i, 2, i - 1)  # the original order, for stability checks
        ws.cell(i, 3, f"=B{i}*2")
        ws.cell(i, 4, f"=B{i}*$F$1")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "out",
        nargs="?",
        default=str(ROOT / "crates/sheets_engine/tests/fixtures/tables.xlsx"),
    )
    args = ap.parse_args()
    wb = Workbook()
    sales(wb)
    plain(wb)
    sort_sheet(wb)
    wb.save(args.out)
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
