#!/usr/bin/env python3
"""Write the fixture for printing charts and pictures (CC0, synthetic):
tests/corpus/print/drawings.xlsx, read by crates/sheets_pdf/tests/drawings.rs.

Sheets (rows are 20 pt, so page splits can be worked out by hand):

* "Split": A4 portrait, A1:F60 of data, row 1 repeated on every page
  (print titles), a manual page break before row 31 (two pages). A column
  chart "Split chart" over B20:F42 reaches across the break (its title on
  page 1, its legend "Legend Q" on page 2). A small PNG logo sits in the
  title row A1 (printed once: objects don't repeat with titles). A JPEG
  photo on page 2 (D45). Two more PNGs that must not print: one hidden
  (`cNvPr hidden="1"`), one with `clientData fPrintsWithSheet="0"`.
* "Hidden": a chart over B2:F20 whose rows 5-9 are hidden (the chart
  prints shorter, as Excel prints it).
* "Fit": A1:D10 of data and a chart "Fit chart" to the right (H2), no
  print area, fit to 1 x 1 page: the printed range grows to take the chart
  in, as Excel's does.
* "Area": print area A1:D20 and a pie chart "Area chart" from C10 reaching
  past it, clipped at the area's edge (its centred title falls outside, so
  it isn't printed).

    uv run --with xlsxwriter==3.3.1 --with pillow==11.3.0 \\
        scripts/corpus/gen_print_drawings_xlsx.py [OUT.xlsx]
"""

import datetime as dt
import io
import re
import struct
import sys
import zipfile
import zlib
from pathlib import Path

import xlsxwriter
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
CREATED = dt.datetime(2026, 11, 11, 12, 0, 0)


def png(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    """A plain PNG of one colour with a darker diagonal."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    rows = []
    for y in range(height):
        row = b"\x00"
        for x in range(width):
            row += bytes(c // 2 for c in rgb) if x == y else bytes(rgb)
        rows.append(row)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
            + chunk(b"IEND", b""))


def jpeg(width: int, height: int) -> bytes:
    """A gradient photo stand-in (baseline JPEG)."""
    img = Image.new("RGB", (width, height))
    img.putdata([(x * 255 // width, y * 255 // height, 120) for y in range(height) for x in range(width)])
    out = io.BytesIO()
    img.save(out, "JPEG", quality=85)
    return out.getvalue()


def data(ws, rows: int, cols: int, bold) -> None:
    heads = ["Item", "Amount", "Qty", "Price", "Region", "Note"][:cols]
    ws.write_row(0, 0, heads, bold)
    for r in range(1, rows):
        ws.write(r, 0, f"Item {r}")
        for c in range(1, cols):
            ws.write_number(r, c, (r * 7 + c * 13) % 97)


def build(path: Path) -> None:
    wb = xlsxwriter.Workbook(str(path), {"in_memory": True})
    wb.set_properties({"title": "drawings.xlsx", "author": "Kantor corpus", "created": CREATED})
    bold = wb.add_format({"bold": True})

    # Split: two pages, a chart across the break.
    ws = wb.add_worksheet("Split")
    ws.set_paper(9)
    ws.set_portrait()
    ws.set_margins(left=0.5, right=0.5, top=0.5, bottom=0.5)
    ws.repeat_rows(0)
    ws.set_h_pagebreaks([30])
    ws.set_default_row(20)
    for r in range(60):
        ws.set_row(r, 20)
    ws.set_column(0, 5, 12)
    data(ws, 60, 6, bold)
    chart = wb.add_chart({"type": "column"})
    chart.add_series({"name": "Legend Q", "categories": "=Split!$A$21:$A$30",
                      "values": "=Split!$B$21:$B$30"})
    chart.set_title({"name": "Split chart"})
    chart.set_legend({"position": "bottom"})
    # 576 px tall: about 21 rows, so rows 20-41.
    ws.insert_chart("B20", chart, {"y_scale": 2.0, "object_position": 1})
    ws.insert_image("A1", "logo.png", {"image_data": io.BytesIO(png(16, 16, (200, 40, 40))),
                                       "description": "Logo in the title row"})
    ws.insert_image("D45", "photo.jpg", {"image_data": io.BytesIO(jpeg(120, 80)),
                                         "description": "Photo on page 2"})
    ws.insert_image("B5", "hidden.png", {"image_data": io.BytesIO(png(20, 20, (40, 200, 40))),
                                         "description": "Hidden picture"})
    ws.insert_image("B12", "noprint.png", {"image_data": io.BytesIO(png(20, 20, (40, 40, 200))),
                                           "description": "Not printed"})

    # Hidden: rows under a chart are hidden.
    ws = wb.add_worksheet("Hidden")
    ws.set_paper(9)
    for r in range(25):
        ws.set_row(r, 20, None, {"hidden": 4 <= r <= 8})
    ws.set_column(0, 5, 12)
    data(ws, 25, 4, bold)
    chart = wb.add_chart({"type": "line"})
    chart.add_series({"name": "Hidden rows", "values": "=Hidden!$B$2:$B$20"})
    chart.set_title({"name": "Hidden chart"})
    ws.insert_chart("B2", chart, {"object_position": 1})

    # Fit: a chart to the right of the data, fit to one page.
    ws = wb.add_worksheet("Fit")
    ws.set_paper(9)
    ws.fit_to_pages(1, 1)
    for r in range(25):
        ws.set_row(r, 20)
    data(ws, 10, 4, bold)
    chart = wb.add_chart({"type": "bar"})
    chart.add_series({"name": "Fit series", "categories": "=Fit!$A$2:$A$10",
                      "values": "=Fit!$B$2:$B$10"})
    chart.set_title({"name": "Fit chart"})
    ws.insert_chart("H2", chart, {"object_position": 1})

    # Area: a chart reaching past the print area.
    ws = wb.add_worksheet("Area")
    ws.set_paper(9)
    ws.print_area("A1:D20")
    for r in range(30):
        ws.set_row(r, 20)
    data(ws, 20, 4, bold)
    chart = wb.add_chart({"type": "pie"})
    chart.add_series({"name": "Area series", "categories": "=Area!$A$2:$A$6",
                      "values": "=Area!$B$2:$B$6"})
    chart.set_title({"name": "Area chart"})
    ws.insert_chart("C10", chart, {"object_position": 1})
    wb.close()
    patch(path)


def patch(path: Path) -> None:
    """Mark one picture hidden and one not printed (XlsxWriter can't)."""
    with zipfile.ZipFile(path) as z:
        parts = [(i, z.read(i.filename)) for i in z.infolist()]
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for info, body in parts:
            if info.filename == "xl/drawings/drawing1.xml":
                body, n = re.subn(rb'(<xdr:cNvPr [^>]*descr="Hidden picture")', rb'\1 hidden="1"', body)
                assert n == 1, "hidden picture"
                anchors = body.split(b"</xdr:twoCellAnchor>")
                k = [i for i, a in enumerate(anchors) if b'descr="Not printed"' in a]
                assert len(k) == 1, "non-printing picture"
                anchors[k[0]], n = re.subn(rb"<xdr:clientData/>",
                                           b'<xdr:clientData fPrintsWithSheet="0"/>', anchors[k[0]])
                assert n == 1
                body = b"</xdr:twoCellAnchor>".join(anchors)
            z.writestr(info, body)
    path.write_bytes(out.getvalue())


def main() -> None:
    if xlsxwriter.__version__ != "3.3.1":
        sys.exit(f"needs XlsxWriter 3.3.1, found {xlsxwriter.__version__}")
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "tests" / "corpus" / "print" / "drawings.xlsx"
    out.parent.mkdir(parents=True, exist_ok=True)
    build(out)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
