#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/25_chartex.xlsx: the Office 2016 chart types
(waterfall, funnel, treemap, sunburst, histogram, Pareto, box & whisker)
as Excel writes them: a chartex part (`xl/charts/chartExN.xml`, from
scripts/corpus/chartex_parts.py) shown by a graphic frame inside
`mc:AlternateContent`, whose fallback is Excel's "This chart isn't
available" box.

One sheet per chart, named after it, holding the chart's data from A1
and the chart to its right. Standard library only (no writer library
makes chartex parts). CC0-1.0 like the rest of the corpus. Deterministic:
`--check` compares parts.

    python3 scripts/corpus/gen_chartex_xlsx.py           # write
    python3 scripts/corpus/gen_chartex_xlsx.py --check   # CI
"""

from __future__ import annotations

import os
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chartex_parts as cxp  # noqa: E402
from gen_shapes_xlsx import HEAD, NS_A, NS_MAIN, NS_PKG, NS_R, NS_XDR, REL, anchor, theme  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "tests", "corpus", "xlsx", "25_chartex.xlsx")
NS_MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"


def sheet(rows: list[list]) -> str:
    def cell(r: int, c: int, v) -> str:
        ref = f"{chr(65 + c)}{r + 1}"
        if isinstance(v, (int, float)):
            return f'<c r="{ref}"><v>{v}</v></c>'
        if v == "":
            return ""
        return f'<c r="{ref}" t="inlineStr"><is><t>{cxp.esc(v)}</t></is></c>'

    data = "".join(
        f'<row r="{r + 1}">' + "".join(cell(r, c, v) for c, v in enumerate(row)) + "</row>"
        for r, row in enumerate(rows)
    )
    return HEAD + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData>{data}</sheetData><drawing r:id="rId1"/></worksheet>'


def drawing(req: str) -> str:
    frame = (
        '<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/>'
        '</xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>'
        f'<a:graphic><a:graphicData uri="{cxp.URI_CHARTEX}"><cx:chart xmlns:cx="{cxp.NS_CX}" r:id="rId1"/></a:graphicData></a:graphic>'
        "</xdr:graphicFrame>"
    )
    fallback = (
        '<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr>'
        '<xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="4572000" cy="2743200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
        '<a:solidFill><a:prstClr val="white"/></a:solidFill><a:ln w="1"><a:solidFill><a:prstClr val="green"/></a:solidFill></a:ln></xdr:spPr>'
        '<xdr:txBody><a:bodyPr vertOverflow="clip" horzOverflow="clip"/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1100"/>'
        "<a:t>This chart isn't available in your version of Excel.</a:t></a:r></a:p></xdr:txBody></xdr:sp>"
    )
    prefix = {cxp.CX2: "cx2", cxp.CX4: "cx4"}.get(req, "cx1")
    return (
        HEAD + f'<xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="{NS_A}" xmlns:r="{NS_R}">'
        f'<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:{prefix}="{req}" Requires="{prefix}">'
        + anchor(4, 1, 12, 17, frame)
        + "</mc:Choice><mc:Fallback>"
        + anchor(4, 1, 12, 17, fallback)
        + "</mc:Fallback></mc:AlternateContent></xdr:wsDr>"
    )


def parts() -> dict[str, str]:
    n = len(cxp.CHARTS)
    ct = (
        HEAD + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
        '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>'
    )
    out: dict[str, str] = {}
    sheets = ""
    wb_rels = ""
    for i, (title, rows, make, req) in enumerate(cxp.CHARTS):
        k = i + 1
        ct += (
            f'<Override PartName="/xl/worksheets/sheet{k}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            f'<Override PartName="/xl/drawings/drawing{k}.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/>'
            f'<Override PartName="/xl/charts/chartEx{k}.xml" ContentType="{cxp.CT_CHARTEX}"/>'
        )
        sheets += f'<sheet name="{title}" sheetId="{k}" r:id="rId{k}"/>'
        wb_rels += f'<Relationship Id="rId{k}" Type="{REL}worksheet" Target="worksheets/sheet{k}.xml"/>'
        out[f"xl/worksheets/sheet{k}.xml"] = sheet(rows)
        out[f"xl/worksheets/_rels/sheet{k}.xml.rels"] = (
            HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}drawing" Target="../drawings/drawing{k}.xml"/></Relationships>'
        )
        out[f"xl/drawings/drawing{k}.xml"] = drawing(req)
        out[f"xl/drawings/_rels/drawing{k}.xml.rels"] = (
            HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{cxp.REL_CHARTEX}" Target="../charts/chartEx{k}.xml"/></Relationships>'
        )
        quoted = f"'{title}'" if " " in title else title
        out[f"xl/charts/chartEx{k}.xml"] = make().replace("Data!", f"{quoted}!")
    out["[Content_Types].xml"] = ct + "</Types>"
    out["_rels/.rels"] = (
        HEAD + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="xl/workbook.xml"/></Relationships>'
    )
    out["xl/workbook.xml"] = HEAD + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets>{sheets}</sheets></workbook>'
    out["xl/_rels/workbook.xml.rels"] = (
        HEAD + f'<Relationships xmlns="{NS_PKG}">{wb_rels}'
        f'<Relationship Id="rId{n + 1}" Type="{REL}theme" Target="theme/theme1.xml"/>'
        f'<Relationship Id="rId{n + 2}" Type="{REL}styles" Target="styles.xml"/></Relationships>'
    )
    out["xl/styles.xml"] = (
        HEAD + f'<styleSheet xmlns="{NS_MAIN}"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
        '<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>'
        '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
        '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        '<cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>'
        '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>'
    )
    out["xl/theme/theme1.xml"] = theme()
    return out


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
