#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/20_charts.xlsx: charts and pictures on sheets.

One workbook for showing drawings over the grid (crates/sheets_engine
drawing module, crates/sheets_ui charts layer):

- "Charts": monthly data in A1:D13 with mixed column widths and row heights,
  a frozen first column, and seven charts anchored over the grid: column,
  line, pie, bar, scatter, area, and a combo (columns plus a line on a
  secondary axis), each with a different `editAs` (move and size, move
  only, absolute) where XlsxWriter allows it.
- A picture, cropped: XlsxWriter can't crop, so `a:srcRect` is added to the
  drawing part afterwards (deterministically).
- "Summary": a chart whose series point at the "Charts" sheet, and no data
  of its own.

Written with XlsxWriter (pinned), from data made up here, CC0-1.0 like the
rest of the corpus (tests/corpus/README.md). Deterministic: `--check`
compares unzipped parts.

    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_charts_xlsx.py           # write
    uv run --with xlsxwriter==3.3.1 scripts/corpus/gen_charts_xlsx.py --check   # CI
"""

from __future__ import annotations

import datetime as dt
import io
import os
import re
import struct
import sys
import tempfile
import zipfile
import zlib

import xlsxwriter

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUT = os.path.join(ROOT, "tests", "corpus", "xlsx")
CREATED = dt.datetime(2026, 11, 11, 12, 0, 0)
WANT_VERSION = "3.3.1"
NAME = "20_charts.xlsx"

MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
SALES = [120, 135, 150, 128, 170, 190, 210, 205, 180, 160, 150, 220]
COSTS = [80, 90, 95, 92, 110, 120, 130, 128, 118, 105, 100, 140]


def png(width: int, height: int) -> bytes:
    """A four-quadrant PNG (red, green / blue, yellow), so cropping shows."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    rows = []
    for y in range(height):
        row = b"\x00"
        for x in range(width):
            top, left = y < height // 2, x < width // 2
            rgb = (220, 60, 50) if top and left else (60, 170, 80) if top else \
                (50, 90, 200) if left else (240, 200, 40)
            row += bytes(rgb)
        rows.append(row)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
            + chunk(b"IEND", b""))


def build(path: str) -> None:
    wb = xlsxwriter.Workbook(path, {"in_memory": True})
    wb.set_properties({"title": NAME, "author": "Kantor corpus", "created": CREATED})
    head = wb.add_format({"bold": True})
    pct = wb.add_format({"num_format": "0.0%"})
    money = wb.add_format({"num_format": "#,##0"})

    ws = wb.add_worksheet("Charts")
    ws.write_row(0, 0, ["Month", "Sales", "Costs", "Margin"], head)
    for i, (m, s, c) in enumerate(zip(MONTHS, SALES, COSTS), start=1):
        ws.write(i, 0, m)
        ws.write_number(i, 1, s, money)
        ws.write_number(i, 2, c, money)
        ws.write_formula(i, 3, f"=(B{i + 1}-C{i + 1})/B{i + 1}", pct, (s - c) / s)
    # Mixed widths and heights under the charts.
    ws.set_column("A:A", 10)
    ws.set_column("B:D", 9)
    ws.set_column("F:F", 20)
    ws.set_column("H:H", 4)
    ws.set_row(3, 30)
    ws.set_row(8, 45)
    ws.set_row(20, 8)
    ws.freeze_panes(0, 1)

    def series(chart, col: str, name: str, **extra) -> None:
        chart.add_series({
            "name": f"=Charts!${col}$1",
            "categories": "=Charts!$A$2:$A$13",
            "values": f"=Charts!${col}$2:${col}$13",
            **extra,
        })

    column = wb.add_chart({"type": "column"})
    series(column, "B", "Sales")
    series(column, "C", "Costs")
    column.set_title({"name": "Sales and costs"})
    ws.insert_chart("F2", column, {"x_scale": 1.0, "y_scale": 1.0})

    line = wb.add_chart({"type": "line"})
    series(line, "B", "Sales", marker={"type": "circle"})
    line.set_title({"name": "Sales"})
    ws.insert_chart("N2", line, {"object_position": 2})

    pie = wb.add_chart({"type": "pie"})
    pie.add_series({
        "name": "Q1 sales",
        "categories": "=Charts!$A$2:$A$4",
        "values": "=Charts!$B$2:$B$4",
        "data_labels": {"percentage": True},
    })
    pie.set_title({"name": "Q1 split"})
    ws.insert_chart("F20", pie, {"x_scale": 0.8, "object_position": 3})

    bar = wb.add_chart({"type": "bar"})
    series(bar, "C", "Costs")
    bar.set_title({"name": "Costs"})
    bar.set_legend({"none": True})
    ws.insert_chart("N20", bar, {"x_offset": 12, "y_offset": 6})

    scatter = wb.add_chart({"type": "scatter"})
    scatter.add_series({
        "name": "Sales vs costs",
        "categories": "=Charts!$C$2:$C$13",
        "values": "=Charts!$B$2:$B$13",
        "marker": {"type": "diamond"},
    })
    scatter.set_title({"name": "Sales vs costs"})
    scatter.set_x_axis({"name": "Costs"})
    scatter.set_y_axis({"name": "Sales"})
    ws.insert_chart("F38", scatter)

    area = wb.add_chart({"type": "area", "subtype": "stacked"})
    series(area, "B", "Sales")
    series(area, "C", "Costs")
    area.set_title({"name": "Stacked"})
    ws.insert_chart("N38", area)

    combo = wb.add_chart({"type": "column"})
    series(combo, "B", "Sales")
    margin = wb.add_chart({"type": "line"})
    series(margin, "D", "Margin", y2_axis=True, marker={"type": "square"})
    combo.combine(margin)
    combo.set_title({"name": "Sales and margin"})
    combo.set_y2_axis({"num_format": "0%"})
    ws.insert_chart("F56", combo, {"x_scale": 1.3})

    ws.insert_image("A16", "quadrants.png", {
        "image_data": io.BytesIO(png(40, 40)),
        "x_scale": 1.5,
        "y_scale": 1.5,
        "description": "Cropped quadrants",
    })

    summary = wb.add_worksheet("Summary")
    summary.write("A1", "Totals live on Charts")
    other = wb.add_chart({"type": "column"})
    other.add_series({
        "name": "Margin",
        "categories": "=Charts!$A$2:$A$13",
        "values": "=Charts!$D$2:$D$13",
    })
    other.set_title({"name": "Margin by month"})
    other.set_y_axis({"num_format": "0%"})
    summary.insert_chart("B3", other)
    wb.close()
    crop_picture(path)


def crop_picture(path: str) -> None:
    """Cut the left quarter and top quarter off the picture (`a:srcRect`)."""
    with zipfile.ZipFile(path) as z:
        parts = [(i, z.read(i.filename)) for i in z.infolist()]
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for info, data in parts:
            if info.filename == "xl/drawings/drawing1.xml":
                data, n = re.subn(rb"(<a:blip [^>]*/>)(<a:stretch>)",
                                  rb'\1<a:srcRect l="25000" t="25000"/>\2', data)
                assert n == 1, "expected one picture"
            z.writestr(info, data)
    with open(path, "wb") as fh:
        fh.write(out.getvalue())


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
            print(f"run: uv run --with xlsxwriter=={WANT_VERSION} scripts/corpus/gen_charts_xlsx.py")
            sys.exit(1)
        print("charts corpus is up to date")
        return
    out = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    os.makedirs(out, exist_ok=True)
    build(os.path.join(out, NAME))
    print(f"wrote {NAME}")


if __name__ == "__main__":
    main()
