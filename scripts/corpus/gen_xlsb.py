#!/usr/bin/env python3
"""Write tests/corpus/xls/styled.xlsb (CC0, synthetic), an Excel binary
workbook built record by record with the standard library only
(deterministic: same bytes every run):

* Sheet "Data": A1 "Quarterly sales" in bold red 14 pt Arial, centred and
  merged over A1:C1, on a 30 pt row; A2:B2 headers with a thin bottom
  border; B3:B4 numbers in a custom "#,##0.00" format, B4 on a yellow fill;
  C3 a percentage (built-in format 10), right-aligned with wrapping; column
  A 20 characters wide, B 15, D hidden; a second merge B6:C7.
* Sheet "Plain": values only, default widths.

Regenerate with `python3 scripts/corpus/gen_xlsb.py`; `--check` compares
with the file in the repository.
"""
import io
import struct
import sys
import zipfile
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / "tests/corpus/xls/styled.xlsb"
STAMP = (2026, 11, 17, 0, 0, 0)


def var(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def rec(ty, data=b""):
    return var(ty) + var(len(data)) + data


def wide(s):
    units = s.encode("utf-16-le")
    return struct.pack("<I", len(units) // 2) + units


def color_rgb(r, g, b):
    return bytes([(2 << 1) | 1, 0]) + struct.pack("<h", 0) + bytes([r, g, b, 255])


AUTO = bytes([1, 0, 0, 0, 0, 0, 0, 0])


def font(height_pt, bold=False, color=AUTO, name="Arial"):
    return rec(
        43,
        struct.pack("<HHHHBBBB", int(height_pt * 20), 0, 700 if bold else 400, 0, 0, 2, 0, 0)
        + color
        + bytes([0])
        + wide(name),
    )


def fill(pattern, fore=AUTO, back=AUTO):
    return rec(45, struct.pack("<I", pattern) + fore + back + struct.pack("<I", 0) + bytes(40) + struct.pack("<I", 0))


def border(bottom=0):
    side = lambda dg: bytes([dg, 0]) + AUTO  # noqa: E731
    return rec(46, bytes([0]) + side(0) + side(bottom) + side(0) + side(0) + side(0))


def xf(fmt=0, font_=0, fill_=0, border_=0, halign=0, valign=2, wrap=False):
    flags = halign | valign << 3 | (0x40 if wrap else 0) | 0x1000  # locked
    return rec(47, struct.pack("<HHHHHBBHBB", 0xFFFF, fmt, font_, fill_, border_, 0, 0, flags, 0, 0))


def styles():
    out = rec(278)  # BrtBeginStyleSheet
    out += rec(615, struct.pack("<I", 1)) + rec(44, struct.pack("<H", 164) + wide("#,##0.00")) + rec(616)
    out += rec(611, struct.pack("<I", 2))
    out += font(10) + font(14, bold=True, color=color_rgb(0xC0, 0, 0))
    out += rec(612)
    out += rec(603, struct.pack("<I", 3))
    out += fill(0) + fill(17) + fill(1, fore=color_rgb(0xFF, 0xFF, 0x00))
    out += rec(604)
    out += rec(613, struct.pack("<I", 2)) + border() + border(bottom=1) + rec(614)
    out += rec(626, struct.pack("<I", 1)) + xf() + rec(627)
    out += rec(617, struct.pack("<I", 6))
    out += xf()  # 0 default
    out += xf(font_=1, halign=2, valign=1)  # 1 title
    out += xf(border_=1)  # 2 header
    out += xf(fmt=164)  # 3 amount
    out += xf(fmt=164, fill_=2)  # 4 amount on yellow
    out += xf(fmt=10, halign=3, wrap=True)  # 5 percent
    out += rec(618)
    out += rec(279)  # BrtEndStyleSheet
    return out


def cell(col, style):
    return struct.pack("<II", col, style & 0xFFFFFF)


def row_hdr(r, height_pt=None):
    flags11 = 0x20 if height_pt else 0  # fUnsynced: custom height
    miy = int((height_pt or 15) * 20)
    return rec(0, struct.pack("<IIHBBBI", r, 0, miy, 0, flags11, 0, 0))


def st(col, style, s):
    return rec(6, cell(col, style) + wide(s))


def real(col, style, v):
    return rec(5, cell(col, style) + struct.pack("<d", v))


def col_info(first, last, width, hidden=False):
    flags = (1 if hidden else 0) | 2
    return rec(60, struct.pack("<IIIIH", first, last, int(width * 256), 0, flags))


def merge(r1, r2, c1, c2):
    return rec(176, struct.pack("<IIII", r1, r2, c1, c2))


def sheet(dim, cols, rows, merges):
    out = rec(129)  # BrtBeginSheet
    out += rec(148, struct.pack("<IIII", *dim))  # BrtWsDim
    out += rec(485, struct.pack("<IHHHH", 0xFFFFFFFF, 8, 300, 0, 0))  # BrtWsFmtInfo
    if cols:
        out += rec(390) + b"".join(cols) + rec(391)
    out += rec(145)  # BrtBeginSheetData
    out += b"".join(rows)
    out += rec(146)  # BrtEndSheetData
    if merges:
        out += rec(177, struct.pack("<I", len(merges))) + b"".join(merges) + rec(178)
    out += rec(130)  # BrtEndSheet
    return out


def data_sheet():
    rows = [
        row_hdr(0, 30) + st(0, 1, "Quarterly sales"),
        row_hdr(1) + st(0, 2, "Item") + st(1, 2, "Amount"),
        row_hdr(2) + st(0, 0, "North") + real(1, 3, 1234.5) + real(2, 5, 0.25),
        row_hdr(3) + st(0, 0, "South") + real(1, 4, 987.25),
        row_hdr(5) + st(1, 0, "Merged block"),
    ]
    cols = [col_info(0, 0, 20), col_info(1, 1, 15), col_info(3, 3, 9, hidden=True)]
    return sheet((0, 6, 0, 3), cols, rows, [merge(0, 0, 0, 2), merge(5, 6, 1, 2)])


def plain_sheet():
    rows = [row_hdr(0) + st(0, 0, "Plain") + real(1, 0, 42)]
    return sheet((0, 0, 0, 1), [], rows, [])


def workbook():
    def bundle(tab, rel, name):
        return rec(156, struct.pack("<II", 0, tab) + wide(rel) + wide(name))

    return (
        rec(131)
        + rec(143)
        + bundle(1, "rId1", "Data")
        + bundle(2, "rId2", "Plain")
        + rec(144)
        + rec(132)
    )


CT = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/>'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/xl/worksheets/sheet1.bin" ContentType="application/vnd.ms-excel.worksheet"/>'
    '<Override PartName="/xl/worksheets/sheet2.bin" ContentType="application/vnd.ms-excel.worksheet"/>'
    '<Override PartName="/xl/styles.bin" ContentType="application/vnd.ms-excel.styles"/>'
    "</Types>"
)
ROOT_RELS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.bin"/>'
    "</Relationships>"
)
WB_RELS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/>'
    '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.bin"/>'
    '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.bin"/>'
    "</Relationships>"
)


def build():
    parts = [
        ("[Content_Types].xml", CT.encode()),
        ("_rels/.rels", ROOT_RELS.encode()),
        ("xl/_rels/workbook.bin.rels", WB_RELS.encode()),
        ("xl/workbook.bin", workbook()),
        ("xl/styles.bin", styles()),
        ("xl/worksheets/sheet1.bin", data_sheet()),
        ("xl/worksheets/sheet2.bin", plain_sheet()),
    ]
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts:
            info = zipfile.ZipInfo(name, STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    return buf.getvalue()


def main():
    data = build()
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_bytes() != data:
            sys.exit(f"{OUT} is out of date: run {sys.argv[0]}")
        print(f"{OUT.name} up to date")
        return
    OUT.write_bytes(data)
    print(OUT)


if __name__ == "__main__":
    main()
