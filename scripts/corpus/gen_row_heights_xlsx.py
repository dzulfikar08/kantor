#!/usr/bin/env python3
"""Write the small row-height fixtures (CC0, synthetic):

* crates/sheets_engine/tests/fixtures/row_heights.xlsx, read by
  crates/sheets_engine/tests/rows.rs;
* with --sample OUT, a bigger sheet for screenshots of the grid (wrapped
  text, a tall row, a hidden row, a merge over rows of different heights).

    uv run --with openpyxl scripts/corpus/gen_row_heights_xlsx.py [--sample OUT.xlsx]
"""

import argparse
from pathlib import Path

from openpyxl import Workbook
from openpyxl.styles import Alignment, Font, PatternFill

ROOT = Path(__file__).resolve().parents[2]


def fixture(path: Path) -> None:
    wb = Workbook()
    ws = wb.active
    ws.title = "Rows"
    ws.sheet_format.defaultRowHeight = 18
    ws.sheet_format.customHeight = True
    ws["A1"] = "default"
    ws["A2"] = "tall"
    ws.row_dimensions[2].height = 40  # customHeight="1"
    ws["A3"] = "hidden"
    ws.row_dimensions[3].hidden = True
    ws["A4"] = "a long text that wraps over several lines in a narrow column"
    ws["A4"].alignment = Alignment(wrap_text=True)
    ws["A5"] = "big"
    ws["A5"].font = Font(size=24)
    ws.merge_cells("B1:B3")
    ws["B1"] = "merged"
    wb.save(path)


def sample(path: Path) -> None:
    wb = Workbook()
    ws = wb.active
    ws.title = "Heights"
    ws.column_dimensions["A"].width = 14
    ws.column_dimensions["B"].width = 26
    ws.column_dimensions["C"].width = 18
    ws.column_dimensions["D"].width = 16
    head = PatternFill("solid", fgColor="DDEBF7")
    for col, title in enumerate(["Item", "Notes", "Merged", "Amount"], start=1):
        c = ws.cell(row=1, column=col, value=title)
        c.font = Font(bold=True)
        c.fill = head
    ws["A2"] = "Wrapped"
    ws["B2"] = "This note is long enough to wrap onto three or four lines in its cell."
    ws["B2"].alignment = Alignment(wrap_text=True, vertical="top")
    ws["A3"] = "Tall row"
    ws["B3"] = "Custom height 48 pt"
    ws.row_dimensions[3].height = 48
    ws["A4"] = "Hidden row"
    ws.row_dimensions[4].hidden = True
    ws["A5"] = "After hidden"
    ws["B5"] = "Row 4 is hidden above"
    ws["A6"] = "Big font"
    ws["B6"] = "Heading"
    ws["B6"].font = Font(size=20, bold=True)
    ws["A7"] = "Short"
    ws.row_dimensions[7].height = 12
    ws["C2"] = "Merge over rows 2-7"
    ws["C2"].alignment = Alignment(horizontal="center", vertical="center", wrap_text=True)
    ws["C2"].fill = PatternFill("solid", fgColor="FFF2CC")
    ws.merge_cells("C2:C7")
    for r in range(2, 30):
        ws.cell(row=r, column=4, value=r * 1250)
    for r in range(8, 30):
        ws.cell(row=r, column=1, value=f"Row {r}")
    wb.save(path)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample", type=Path)
    args = ap.parse_args()
    if args.sample:
        sample(args.sample)
        print(f"wrote {args.sample}")
        return
    out = ROOT / "crates/sheets_engine/tests/fixtures/row_heights.xlsx"
    fixture(out)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
