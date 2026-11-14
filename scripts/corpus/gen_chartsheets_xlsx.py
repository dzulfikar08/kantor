#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/22_chart_sheets.xlsx: chart sheets between worksheets.

Tab order: "Revenue chart" (a chart sheet before every worksheet: a column
chart), "Data" (the numbers, active), "Share" (a chart sheet: a pie of the
Data sheet), "Hidden chart" (a hidden chart sheet), "Notes" (a worksheet
after the chart sheets). Each chart sheet holds one chart in its drawing
(`xl/chartsheets/sheetN.xml` → drawing → chart), as Excel writes them.

Written with XlsxWriter (pinned), from data made up here, CC0-1.0 like the
rest of the corpus (tests/corpus/README.md). Deterministic: `--check`
compares unzipped parts.

    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_chartsheets_xlsx.py           # write
    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_chartsheets_xlsx.py --check   # CI
"""

from __future__ import annotations

import datetime as dt
import os
import sys
import tempfile
import zipfile

import xlsxwriter

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUT = os.path.join(ROOT, "tests", "corpus", "xlsx")
NAME = "22_chart_sheets.xlsx"
WANT_VERSION = "3.3.1"
CREATED = dt.datetime(2026, 11, 14, 9, 0, 0)

REGIONS = ["North", "South", "East", "West"]
Q1 = [120, 95, 143, 88]
Q2 = [134, 101, 150, 97]


def build(path: str) -> None:
    wb = xlsxwriter.Workbook(path, {"in_memory": True})
    wb.set_properties({"title": NAME, "author": "Kantor corpus", "created": CREATED})

    revenue = wb.add_chartsheet("Revenue chart")
    data = wb.add_worksheet("Data")
    share = wb.add_chartsheet("Share")
    hidden = wb.add_chartsheet("Hidden chart")
    notes = wb.add_worksheet("Notes")

    data.write_row("A1", ["Region", "Q1", "Q2", "Total"])
    for i, (r, a, b) in enumerate(zip(REGIONS, Q1, Q2), start=2):
        data.write(f"A{i}", r)
        data.write_number(f"B{i}", a)
        data.write_number(f"C{i}", b)
        data.write_formula(f"D{i}", f"=B{i}+C{i}", None, a + b)
    data.set_column("A:A", 12)
    data.activate()

    column = wb.add_chart({"type": "column"})
    for col, name in (("B", "Q1"), ("C", "Q2")):
        column.add_series({
            "name": f"=Data!${col}$1",
            "categories": "=Data!$A$2:$A$5",
            "values": f"=Data!${col}$2:${col}$5",
        })
    column.set_title({"name": "Revenue by region"})
    revenue.set_chart(column)

    pie = wb.add_chart({"type": "pie"})
    pie.add_series({
        "name": "Total",
        "categories": "=Data!$A$2:$A$5",
        "values": "=Data!$D$2:$D$5",
        "data_labels": {"percentage": True},
    })
    pie.set_title({"name": "Share of total"})
    share.set_chart(pie)

    line = wb.add_chart({"type": "line"})
    line.add_series({"name": "Q1", "categories": "=Data!$A$2:$A$5", "values": "=Data!$B$2:$B$5"})
    hidden.set_chart(line)
    hidden.hide()

    notes.write("A1", "Chart sheets show the Data sheet")
    wb.close()


def unpacked(path: str) -> dict[str, bytes] | None:
    if not os.path.exists(path):
        return None
    with zipfile.ZipFile(path) as z:
        return {n: z.read(n) for n in z.namelist()}


def main() -> None:
    if xlsxwriter.__version__ != WANT_VERSION:
        sys.exit(f"needs XlsxWriter {WANT_VERSION} (output differs between versions), "
                 f"found {xlsxwriter.__version__}")
    if sys.argv[1:] == ["--check"]:
        with tempfile.TemporaryDirectory() as tmp:
            build(os.path.join(tmp, NAME))
            same = unpacked(os.path.join(tmp, NAME)) == unpacked(os.path.join(DEFAULT_OUT, NAME))
        if not same:
            print(f"out of date: tests/corpus/xlsx/{NAME}")
            print(f"run: uv run --with xlsxwriter=={WANT_VERSION} scripts/corpus/gen_chartsheets_xlsx.py")
            sys.exit(1)
        print("chart sheets corpus is up to date")
        return
    out = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    os.makedirs(out, exist_ok=True)
    build(os.path.join(out, NAME))
    print(f"wrote {NAME}")


if __name__ == "__main__":
    main()
