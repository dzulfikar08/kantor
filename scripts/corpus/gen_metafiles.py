#!/usr/bin/env python3
"""Write synthetic legacy pictures (CC0) with the standard library only,
record by record (deterministic: same bytes every run), into
tests/corpus/metafile:

* shapes.wmf: a placeable Windows Metafile: a light blue background, a red
  ellipse with a dark outline, a green triangle, a dashed line, the text
  "WMF" in Arial.
* chart.emf: an Enhanced Metafile drawn the way chart exporters do: three
  bars (solid brushes), axes (a polyline), a pie slice, a hatched box,
  a 2 × 2 bitmap (StretchDIBits) and the title "Sales" (ExtTextOutW).
* plus.emf: an "EMF+ only" metafile: an orange rectangle and a blue
  ellipse from EMF+ records only.
* art.pict: a QuickDraw PICT 2: a filled orange rectangle and a framed oval.
* dib.bmp: a 4 × 2 24-bit bitmap.
* metafiles.docx: a document with chart.emf inline and shapes.wmf.

With `--soffice PATH` (LibreOffice), also flat ODF files holding
chart.emf and shapes.wmf are converted to tests/corpus/doc/metafiles.doc,
tests/corpus/xls/metafiles.xls and metafiles.ods, and
tests/corpus/ppt/metafiles.ppt (LibreOffice's bytes differ per run: those
aren't checked).

Regenerate with `python3 scripts/corpus/gen_metafiles.py`; `--check`
compares the metafiles and metafiles.docx with the files in the repository.
"""
import base64
import io
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests/corpus/metafile"
STAMP = (2026, 11, 17, 0, 0, 0)


def colorref(r, g, b):
    return r | g << 8 | b << 16


# ---- WMF ---------------------------------------------------------------------


def wmf_rec(func, *params):
    body = b"".join(struct.pack("<h", p) if isinstance(p, int) else p for p in params)
    if len(body) % 2:
        body += b"\0"
    return struct.pack("<IH", 3 + len(body) // 2, func) + body


def wmf_color(c):
    return struct.pack("<I", c)


def shapes_wmf():
    recs = [
        wmf_rec(0x0103, 8),  # SetMapMode anisotropic
        wmf_rec(0x020B, 0, 0),  # SetWindowOrg y, x
        wmf_rec(0x020C, 1000, 2000),  # SetWindowExt y, x
        wmf_rec(0x0102, 1),  # SetBkMode transparent
        # 0: light blue brush, 1: null pen, 2: red brush, 3: dark pen 20 wide
        struct.pack("<IH", 7, 0x02FC) + struct.pack("<H", 0) + wmf_color(colorref(220, 235, 250)) + struct.pack("<H", 0),
        wmf_rec(0x02FA, 5, 0, 0, wmf_color(0)),
        struct.pack("<IH", 7, 0x02FC) + struct.pack("<H", 0) + wmf_color(colorref(220, 40, 40)) + struct.pack("<H", 0),
        wmf_rec(0x02FA, 0, 20, 0, wmf_color(colorref(40, 40, 40))),
        wmf_rec(0x012D, 0),
        wmf_rec(0x012D, 1),
        wmf_rec(0x041B, 1000, 2000, 0, 0),  # Rectangle b, r, t, l
        wmf_rec(0x012D, 2),
        wmf_rec(0x012D, 3),
        wmf_rec(0x0418, 900, 900, 100, 100),  # Ellipse
        # 4: green brush; a triangle.
        struct.pack("<IH", 7, 0x02FC) + struct.pack("<H", 0) + wmf_color(colorref(30, 160, 60)) + struct.pack("<H", 0),
        wmf_rec(0x012D, 4),
        wmf_rec(0x0324, 3, 1100, 900, 1900, 900, 1500, 100),
        # 5: dashed blue pen; a line.
        wmf_rec(0x02FA, 1, 0, 0, wmf_color(colorref(0, 0, 200))),
        wmf_rec(0x012D, 5),
        wmf_rec(0x0214, 950, 100),  # MoveTo y, x
        wmf_rec(0x0213, 950, 1900),  # LineTo
    ]
    face = b"Arial".ljust(32, b"\0")
    recs.append(wmf_rec(0x02FB, -200, 0, 0, 0, 700, struct.pack("<8B", 0, 0, 0, 0, 0, 0, 0, 0), face))
    recs.append(wmf_rec(0x012D, 6))
    recs.append(wmf_rec(0x0209, wmf_color(colorref(255, 255, 255))))
    recs.append(wmf_rec(0x012E, 24))  # baseline
    recs.append(wmf_rec(0x0521, 3, b"WMF\0", 600, 300))
    recs.append(struct.pack("<IH", 3, 0))
    body = b"".join(recs)
    header = struct.pack("<HHHIHIH", 1, 9, 0x0300, 9 + len(body) // 2, 7, 64, 0)
    place = struct.pack("<IHhhhhHI", 0x9AC6CDD7, 0, 0, 0, 2000, 1000, 1000, 0)
    check = 0
    for i in range(0, len(place), 2):
        check ^= struct.unpack_from("<H", place, i)[0]
    return place + struct.pack("<H", check) + header + body


# ---- EMF ---------------------------------------------------------------------


def emf_rec(ty, data=b""):
    while len(data) % 4:
        data += b"\0"
    return struct.pack("<II", ty, 8 + len(data)) + data


def ints(*v):
    return struct.pack(f"<{len(v)}i", *v)


def emf_file(records, w_px=200, h_px=150, extra_handles=8):
    body = b"".join(records) + emf_rec(14, ints(0, 0, 20))
    mm = lambda px: round(px * 2540 / 96)  # noqa: E731 (.01 mm)
    desc = b""
    header = (
        ints(0, 0, w_px - 1, h_px - 1)
        + ints(0, 0, mm(w_px), mm(h_px))
        + struct.pack("<IIIIHH", 0x464D4520, 0x10000, 0, len(records) + 2, extra_handles, 0)
        + struct.pack("<III", 0, 0, 0)
        + ints(1920, 1080, 508, 286)
    )
    head = emf_rec(1, header + desc)
    total = len(head) + len(body)
    head = head[:48] + struct.pack("<I", total) + head[52:]
    return head + body


def brush(ih, color, style=0, hatch=0):
    return emf_rec(39, struct.pack("<IIII", ih, style, color, hatch))


def select(ih):
    return emf_rec(37, struct.pack("<I", ih))


def chart_emf():
    r = [
        emf_rec(18, ints(1)),  # transparent background
        brush(1, colorref(255, 255, 255)),
        select(1),
        select(0x80000008),  # null pen
        emf_rec(43, ints(0, 0, 200, 150)),
    ]
    for i, (h, c) in enumerate([(60, (68, 114, 196)), (95, (237, 125, 49)), (40, (112, 173, 71))]):
        r += [brush(2 + i, colorref(*c)), select(2 + i)]
        x = 30 + i * 35
        r.append(emf_rec(43, ints(x, 130 - h, x + 25, 130)))
    # Axes: a black pen, a polyline.
    r += [
        emf_rec(38, struct.pack("<IIiiI", 5, 0, 2, 0, colorref(0, 0, 0))),
        select(5),
        emf_rec(4, ints(20, 25, 140, 130) + struct.pack("<I", 3) + ints(20, 25, 20, 130, 140, 130)),
        # A pie slice and a hatched box.
        brush(6, colorref(200, 30, 120)),
        select(6),
        emf_rec(47, ints(150, 40, 195, 85, 195, 62, 172, 40)),
        brush(7, colorref(0, 0, 0), style=2, hatch=4),
        select(7),
        emf_rec(43, ints(150, 95, 195, 125)),
    ]
    # A 2 × 2 bitmap: StretchDIBits at the top right.
    bmi = struct.pack("<IiiHHIIiiII", 40, 2, 2, 1, 24, 0, 0, 0, 0, 0, 0)
    bits = bytes([0, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0, 255, 255, 255, 0, 0])
    head = 80
    rec = ints(150, 5, 190, 30) + ints(150, 5, 0, 0, 2, 2) + struct.pack(
        "<IIIIII", head, len(bmi), head + len(bmi), len(bits), 0, 0x00CC0020
    ) + ints(40, 25) + bmi + bits
    r.append(emf_rec(81, rec))
    # Title: a font and ExtTextOutW.
    face = "Arial".encode("utf-16-le").ljust(64, b"\0")
    r.append(emf_rec(82, struct.pack("<I", 8) + ints(-14, 0, 0, 0, 700) + bytes(8) + face))
    r.append(select(8))
    r.append(emf_rec(24, struct.pack("<I", colorref(30, 30, 30))))
    r.append(emf_rec(22, struct.pack("<I", 24)))
    text = "Sales".encode("utf-16-le")
    t = ints(0, 0, 0, 0) + struct.pack("<iff", 1, 1.0, 1.0)
    t += ints(30, 18) + struct.pack("<III", 5, 76, 0) + ints(0, 0, 0, 0) + struct.pack("<I", 0)
    r.append(emf_rec(84, t + text))
    return emf_file(r)


def plus_rec(ty, flags, data):
    return struct.pack("<HHII", ty, flags, 12 + len(data), len(data)) + data


def plus_emf():
    recs = plus_rec(0x4001, 0, struct.pack("<IIII", 0xDBC01002, 1, 96, 96))
    recs += plus_rec(0x4030, 2, struct.pack("<f", 1.0))
    recs += plus_rec(0x400A, 0x8000, struct.pack("<II4f", 0xFFF08020, 1, 10, 10, 120, 80))
    # A solid brush object (id 1), then FillEllipse with it.
    recs += plus_rec(0x4008, 0x0100 | 1, struct.pack("<III", 0xDBC01002, 0, 0xFF2050D0))
    recs += plus_rec(0x400E, 0, struct.pack("<I4f", 1, 90, 50, 100, 90))
    comment = struct.pack("<II", len(recs) + 4, 0x2B464D45) + recs
    eof = plus_rec(0x4002, 0, b"")
    comment2 = struct.pack("<II", len(eof) + 4, 0x2B464D45) + eof
    return emf_file([emf_rec(70, comment), emf_rec(70, comment2)])


# ---- PICT and BMP ------------------------------------------------------------


def art_pict():
    p = bytes(512) + struct.pack(">H4h", 0, 0, 0, 80, 120)
    p += struct.pack(">HH", 0x0011, 0x02FF) + struct.pack(">H", 0x0C00) + bytes(24)
    p += struct.pack(">H3H", 0x001A, 0xFFFF, 0x8000, 0x0000)
    p += struct.pack(">H4h", 0x0031, 10, 10, 70, 60)  # PaintRect t l b r
    p += struct.pack(">H3H", 0x001A, 0x0000, 0x0000, 0xC000)
    p += struct.pack(">H2h", 0x0007, 3, 3)  # PnSize
    p += struct.pack(">H4h", 0x0050, 10, 65, 70, 115)  # FrameOval
    p += struct.pack(">H", 0x00FF)
    return p


def dib_bmp():
    rows = bytes([255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]) * 2
    info = struct.pack("<IiiHHIIiiII", 40, 4, 2, 1, 24, 0, len(rows), 2835, 2835, 0, 0)
    return b"BM" + struct.pack("<IHHI", 14 + len(info) + len(rows), 0, 0, 54) + info + rows


# ---- metafiles.docx ------------------------------------------------------------


def docx(chart, shapes):
    w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
    ns = (
        f'xmlns:w="{w}" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
        'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"'
    )

    def inline(rid, i, cx, cy):
        return (
            f'<w:r><w:drawing><wp:inline><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{i}" name="Picture {i}"/>'
            '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
            f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{i}" name="Picture {i}"/><pic:cNvPicPr/></pic:nvPicPr>'
            f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
            f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>'
            "</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
        )

    body = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:document {ns}><w:body>'
        "<w:p><w:r><w:t>An EMF chart and a WMF drawing:</w:t></w:r></w:p>"
        f"<w:p>{inline('rId10', 1, 1905000, 1428750)}</w:p>"
        f"<w:p>{inline('rId11', 2, 1905000, 952500)}</w:p>"
        '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>'
        "</w:body></w:document>"
    )
    parts = {
        "[Content_Types].xml": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Default Extension="emf" ContentType="image/x-emf"/><Default Extension="wmf" ContentType="image/x-wmf"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        "</Types>",
        "_rels/.rels": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
        "</Relationships>",
        "word/_rels/document.xml.rels": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.emf"/>'
        '<Relationship Id="rId11" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.wmf"/>'
        "</Relationships>",
        "word/document.xml": body,
        "word/media/image1.emf": chart,
        "word/media/image2.wmf": shapes,
    }
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            info = zipfile.ZipInfo(name, STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data.encode() if isinstance(data, str) else data)
    return buf.getvalue()


# ---- flat ODF for LibreOffice --------------------------------------------------

OFFICE_NS = (
    'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
    'xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" '
    'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
    'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
    'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
    'xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" '
    'xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" '
    'xmlns:xlink="http://www.w3.org/1999/xlink" '
    'xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"'
)


def frame(name, data, x, y, w, h, extra=""):
    b64 = base64.b64encode(data).decode()
    return (
        f'<draw:frame draw:name="{name}" svg:x="{x}" svg:y="{y}" svg:width="{w}" svg:height="{h}"{extra}>'
        f"<draw:image><office:binary-data>{b64}</office:binary-data></draw:image></draw:frame>"
    )


def flat_odf(kind, chart, shapes):
    head = f'<?xml version="1.0" encoding="UTF-8"?>\n<office:document {OFFICE_NS} office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.{kind}"><office:body>'
    tail = "</office:body></office:document>"
    if kind == "text":
        return head + (
            "<office:text><text:p>An EMF chart and a WMF drawing:</text:p><text:p>"
            + frame("Chart", chart, "0cm", "0cm", "5.3cm", "3.97cm", ' text:anchor-type="as-char"')
            + "</text:p><text:p>"
            + frame("Shapes", shapes, "0cm", "0cm", "5.3cm", "2.65cm", ' text:anchor-type="as-char"')
            + "</text:p></office:text>"
        ) + tail
    if kind == "spreadsheet":
        return head + (
            '<office:spreadsheet><table:table table:name="Pictures"><table:shapes>'
            + frame("Chart", chart, "2cm", "1cm", "5.3cm", "3.97cm")
            + frame("Shapes", shapes, "8cm", "1cm", "5.3cm", "2.65cm")
            + "</table:shapes><table:table-column/><table:table-row><table:table-cell office:value-type=\"string\"><text:p>Pictures</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet>"
        ) + tail
    return head + (
        '<office:presentation><draw:page draw:name="Slide 1">'
        + frame("Chart", chart, "2cm", "2cm", "10.6cm", "7.94cm")
        + frame("Shapes", shapes, "14cm", "2cm", "10.6cm", "5.3cm")
        + "</draw:page></office:presentation>"
    ) + tail


def soffice_convert(soffice, src, filt, outdir, profile):
    cmd = [
        str(soffice),
        f"-env:UserInstallation=file://{profile}",
        "--headless",
        "--convert-to",
        filt,
        "--outdir",
        str(outdir),
        str(src),
    ]
    if subprocess.run(cmd).returncode == 81:
        subprocess.run(cmd, check=True)


def main():
    args = sys.argv[1:]
    check = "--check" in args
    soffice = None
    if "--soffice" in args:
        soffice = Path(args[args.index("--soffice") + 1]).resolve()
    chart, shapes = chart_emf(), shapes_wmf()
    files = {
        OUT / "shapes.wmf": shapes,
        OUT / "chart.emf": chart,
        OUT / "plus.emf": plus_emf(),
        OUT / "art.pict": art_pict(),
        OUT / "dib.bmp": dib_bmp(),
        ROOT / "tests/corpus/docx/metafiles.docx": docx(chart, shapes),
    }
    if check:
        bad = [str(p) for p, data in files.items() if not p.exists() or p.read_bytes() != data]
        if bad:
            sys.exit("out of date: " + ", ".join(bad))
        print("metafiles up to date")
        return
    OUT.mkdir(parents=True, exist_ok=True)
    for p, data in files.items():
        p.write_bytes(data)
        print(p)
    if soffice is None:
        return
    base = Path(os.environ.get("TMPDIR", Path.home() / ".cache/ktmp"))
    base.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=base) as t:
        tmp = Path(t)
        profile = tmp / "profile"
        jobs = [
            ("text", "fodt", "doc:MS Word 97", "doc", ROOT / "tests/corpus/doc"),
            ("spreadsheet", "fods", "xls:MS Excel 97", "xls", ROOT / "tests/corpus/xls"),
            ("spreadsheet", "fods", "ods:calc8", "ods", ROOT / "tests/corpus/xls"),
            ("presentation", "fodp", "ppt:MS PowerPoint 97", "ppt", ROOT / "tests/corpus/ppt"),
        ]
        for kind, ext, filt, out_ext, dest in jobs:
            src = tmp / f"metafiles.{ext}"
            src.write_text(flat_odf(kind, chart, shapes), encoding="utf-8")
            soffice_convert(soffice, src, filt, tmp, profile)
            shutil.copy(tmp / f"metafiles.{out_ext}", dest / f"metafiles.{out_ext}")
            print(dest / f"metafiles.{out_ext}")


if __name__ == "__main__":
    main()
