#!/usr/bin/env python3
"""Generate tests/corpus/docx/chartex.docx: a floating waterfall chart (an
Office 2016 chartex part, scripts/corpus/chartex_parts.py) as Word writes
one, in `mc:AlternateContent` with a text fallback, between two paragraphs.
Read by crates/docs_pdf/tests/floats.rs. Standard library only. CC0-1.0.
Deterministic: `--check` compares parts.

    python3 scripts/corpus/gen_docx_chartex.py           # write
    python3 scripts/corpus/gen_docx_chartex.py --check   # CI
"""

from __future__ import annotations

import os
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chartex_parts as cxp  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "tests", "corpus", "docx", "chartex.docx")
HEAD = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
NS_W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
NS_WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
NS_MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"
NS_PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
CX, CY = 4572000, 2743200


def anchor(graphic: str) -> str:
    return (
        '<w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="251659264" '
        'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
        '<wp:positionH relativeFrom="column"><wp:align>center</wp:align></wp:positionH>'
        '<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{CX}" cy="{CY}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapTopAndBottom/>'
        '<wp:docPr id="1" name="Chart 1"/><wp:cNvGraphicFramePr/>'
        f"{graphic}</wp:anchor></w:drawing>"
    )


def document() -> str:
    chart = (
        f'<a:graphic xmlns:a="{cxp.NS_A}"><a:graphicData uri="{cxp.URI_CHARTEX}">'
        f'<cx:chart xmlns:cx="{cxp.NS_CX}" r:id="rId1"/></a:graphicData></a:graphic>'
    )
    alt = (
        f'<mc:AlternateContent><mc:Choice xmlns:cx1="{cxp.CX1}" Requires="cx1">{anchor(chart)}</mc:Choice>'
        "<mc:Fallback><w:t>This chart isn't available in your version of Word.</w:t></mc:Fallback></mc:AlternateContent>"
    )
    return (
        HEAD + f'<w:document xmlns:w="{NS_W}" xmlns:wp="{NS_WP}" xmlns:mc="{NS_MC}" xmlns:r="{cxp.NS_R}"><w:body>'
        "<w:p><w:r><w:t>Before the chart.</w:t></w:r></w:p>"
        f"<w:p><w:r>{alt}</w:r><w:r><w:t>After the chart.</w:t></w:r></w:p>"
        '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" '
        'w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>'
    )


def parts() -> dict[str, str]:
    return {
        "[Content_Types].xml": HEAD
        + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        f'<Override PartName="/word/charts/chartEx1.xml" ContentType="{cxp.CT_CHARTEX}"/></Types>',
        "_rels/.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="word/document.xml"/></Relationships>',
        "word/document.xml": document(),
        "word/_rels/document.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{cxp.REL_CHARTEX}" Target="charts/chartEx1.xml"/></Relationships>',
        "word/charts/chartEx1.xml": cxp.waterfall().replace("Data!", "Sheet1!"),
    }


def write(path: str) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts().items():
            info = zipfile.ZipInfo(name, (2026, 11, 16, 9, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, text.encode("utf-8"))


def main() -> int:
    if "--check" in sys.argv:
        with zipfile.ZipFile(OUT) as z:
            have = {n: z.read(n).decode("utf-8") for n in z.namelist()}
        if have != parts():
            print(f"{OUT} is out of date: run {sys.argv[0]}", file=sys.stderr)
            return 1
        return 0
    write(OUT)
    print(OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
