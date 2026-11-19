#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/28_shaped_pictures.xlsx: pictures cut to a
shape, with and without an outline, as Excel writes them (Picture Format >
Crop to Shape, Picture Border, the picture styles).

Raw SpreadsheetML (standard library only); the picture is a 64 x 48 PNG
drawn here (a diagonal colour ramp). Sheet "Pictures" holds, by name:

* "Oval photo": an ellipse, dark red 3 pt outline.
* "Rounded photo": a rounded rectangle, 2 pt dashed outline in the theme's
  accent 1.
* "Star photo": a five-point star, no outline.
* "Framed photo": a rectangle with a 4 pt dark grey frame, cropped 10 %
  on the left.
* "No line": an ellipse whose outline is `a:noFill`.
* "Styled photo": an ellipse with no `a:ln`, outlined only by its
  `xdr:style` (`a:lnRef idx="3"`, accent 2 at 50 % shade: the theme's
  1.5 pt line).
* "Styled wide": a rectangle whose `a:ln` sets only the width (3 pt), its
  colour from `a:lnRef idx="1"` (accent 6).
* "Styled none": an ellipse with `a:lnRef idx="2"` but `a:ln` `a:noFill`:
  no outline.

Sheet "Other" has no drawing. CC0-1.0 like the rest of the corpus.
Deterministic (fixed zip timestamps): `--check` compares parts.

    python3 scripts/corpus/gen_shaped_pictures_xlsx.py           # write
    python3 scripts/corpus/gen_shaped_pictures_xlsx.py --check   # CI
"""

from __future__ import annotations

import os
import struct
import sys
import zipfile
import zlib

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_shapes_xlsx as base  # noqa: E402  (theme, sheet and anchor writers)

OUT = os.path.join(base.ROOT, "tests", "corpus", "xlsx", "28_shaped_pictures.xlsx")
HEAD = base.HEAD
REL = base.REL
NS_PKG = base.NS_PKG
NS_MAIN = base.NS_MAIN
NS_R = base.NS_R


def png(w: int = 64, h: int = 48) -> bytes:
    """A w x h RGB PNG: red rises to the right, blue down, green fixed."""
    rows = b"".join(
        b"\x00" + b"".join(bytes((x * 255 // (w - 1), 160, y * 255 // (h - 1))) for x in range(w))
        for y in range(h)
    )

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


def pic(id_: int, name: str, geom: str, line: str = "", crop: str = "", style: str = "") -> str:
    return (
        f'<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{id_}" name="{name}"/><xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr></xdr:nvPicPr>'
        f'<xdr:blipFill><a:blip xmlns:r="{NS_R}" r:embed="rId1"/>{crop}<a:stretch><a:fillRect/></a:stretch></xdr:blipFill>'
        f"<xdr:spPr>{base.xfrm()}{geom}{line}</xdr:spPr>{style}</xdr:pic>"
    )


def style(idx: int, color: str) -> str:
    """A picture's `xdr:style`: its outline from theme line style `idx`."""
    return (
        f'<xdr:style><a:lnRef idx="{idx}">{color}</a:lnRef><a:fillRef idx="1"><a:srgbClr val="FFFFFF"/></a:fillRef>'
        '<a:effectRef idx="0"><a:srgbClr val="000000"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="tx1"/></a:fontRef></xdr:style>'
    )


def prst(name: str) -> str:
    return f'<a:prstGeom prst="{name}"><a:avLst/></a:prstGeom>'


def drawing() -> str:
    objs = [
        base.anchor(1, 1, 4, 9, pic(
            2, "Oval photo", prst("ellipse"),
            '<a:ln w="38100"><a:solidFill><a:srgbClr val="C00000"/></a:solidFill></a:ln>',
        )),
        base.anchor(5, 1, 8, 9, pic(
            3, "Rounded photo", prst("roundRect"),
            '<a:ln w="25400"><a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:prstDash val="dash"/></a:ln>',
        )),
        base.anchor(9, 1, 12, 9, pic(4, "Star photo", prst("star5"))),
        base.anchor(1, 11, 4, 19, pic(
            5, "Framed photo", prst("rect"),
            '<a:ln w="50800" cap="sq"><a:solidFill><a:srgbClr val="404040"/></a:solidFill><a:miter lim="800000"/></a:ln>',
            '<a:srcRect l="10000"/>',
        )),
        base.anchor(5, 11, 8, 19, pic(6, "No line", prst("ellipse"), "<a:ln><a:noFill/></a:ln>")),
        base.anchor(1, 21, 4, 29, pic(
            7, "Styled photo", prst("ellipse"),
            style=style(3, '<a:schemeClr val="accent2"><a:shade val="50000"/></a:schemeClr>'),
        )),
        base.anchor(5, 21, 8, 29, pic(
            8, "Styled wide", prst("rect"), '<a:ln w="38100"/>',
            style=style(1, '<a:schemeClr val="accent6"/>'),
        )),
        base.anchor(9, 21, 12, 29, pic(
            9, "Styled none", prst("ellipse"), "<a:ln><a:noFill/></a:ln>",
            style=style(2, '<a:schemeClr val="accent1"/>'),
        )),
    ]
    return HEAD + f'<xdr:wsDr xmlns:xdr="{base.NS_XDR}" xmlns:a="{base.NS_A}">' + "".join(objs) + "</xdr:wsDr>"


def parts() -> dict[str, bytes]:
    shapes = base.parts()
    out = {
        "[Content_Types].xml": shapes["[Content_Types].xml"].replace(
            '<Default Extension="xml" ContentType="application/xml"/>',
            '<Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>',
        ),
        "_rels/.rels": shapes["_rels/.rels"],
        "xl/workbook.xml": shapes["xl/workbook.xml"].replace('name="Shapes"', 'name="Pictures"'),
        "xl/_rels/workbook.xml.rels": shapes["xl/_rels/workbook.xml.rels"],
        "xl/styles.xml": shapes["xl/styles.xml"],
        "xl/theme/theme1.xml": shapes["xl/theme/theme1.xml"],
        "xl/worksheets/sheet1.xml": base.sheet([["Picture", "Shape"], ["Oval photo", "ellipse"], ["Star photo", "star5"]], "rId1"),
        "xl/worksheets/_rels/sheet1.xml.rels": shapes["xl/worksheets/_rels/sheet1.xml.rels"],
        "xl/worksheets/sheet2.xml": base.sheet([["Nothing drawn here"]], None),
        "xl/drawings/drawing1.xml": drawing(),
        "xl/drawings/_rels/drawing1.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}image" Target="../media/image1.png"/></Relationships>',
    }
    enc = {n: t.encode("utf-8") for n, t in out.items()}
    enc["xl/media/image1.png"] = png()
    return enc


def write(path: str) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts().items():
            info = zipfile.ZipInfo(name, (2026, 11, 18, 9, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)


def main() -> int:
    if "--check" in sys.argv:
        with zipfile.ZipFile(OUT) as z:
            have = {n: z.read(n) for n in z.namelist()}
        if have != parts():
            print(f"{OUT} is out of date: run {sys.argv[0]}", file=sys.stderr)
            return 1
        return 0
    write(OUT)
    print(OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
