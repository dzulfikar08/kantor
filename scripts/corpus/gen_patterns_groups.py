#!/usr/bin/env python3
"""Generate the pattern fill and "chart inside a group" fixtures (CC0,
synthetic, standard library only):

* tests/corpus/xlsx/27_patterns_group_charts.xlsx, sheet "Patterns"
  (A1:B5: Region / Sales, North 120, South 95, East 143, West 88):
  - "Diagonal box": a rectangle filled with `dkUpDiag` (blue on white),
    black outline, text "Pattern".
  - "Checker oval": an ellipse filled with `smCheck` (dark red on yellow).
  - "Chart group": a group of the rectangle "Caption" (text "Sales by
    region") and the chart "Grouped chart" (clustered columns of B2:B5 by
    A2:A5, its series filled with `ltUpDiag` green on white).
  - "Plain chart": a chart outside any group whose plot area is filled
    with `pct20` (grey on white) and whose series is solid orange.
* tests/corpus/docx/patterns_group_charts.docx: a floating rectangle
  "Pattern rect" filled with `wdDnDiag` (green on light yellow) and a
  floating group "Chart group" of the shape "Group caption" and the chart
  "Group chart" (its own chart part, values cached, no workbook).

* tests/corpus/xls/patterns_groups.ods and .xls: the .xlsx as LibreOffice
  saves it (`--soffice [LO_APPIMAGE]` writes only these; LibreOffice turns
  the patterns into hatches in .ods and into texture pictures in .xls, and
  keeps the chart inside its group). Not checked by `--check`.

Deterministic (fixed zip timestamps): `--check` compares the .xlsx and
.docx.

    python3 scripts/corpus/gen_patterns_groups.py           # write
    python3 scripts/corpus/gen_patterns_groups.py --check   # CI
    python3 scripts/corpus/gen_patterns_groups.py --soffice ~/.cache/lo/lo.AppImage
"""

from __future__ import annotations

import io
import os
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_shapes_xlsx import (  # noqa: E402
    HEAD,
    NS_A,
    NS_MAIN,
    NS_PKG,
    NS_R,
    NS_XDR,
    REL,
    anchor,
    body,
    para,
    sheet,
    sp,
    theme,
    xfrm,
)

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
XLSX = os.path.join(ROOT, "tests", "corpus", "xlsx", "27_patterns_group_charts.xlsx")
DOCX = os.path.join(ROOT, "tests", "corpus", "docx", "patterns_group_charts.docx")
NS_C = "http://schemas.openxmlformats.org/drawingml/2006/chart"
CHART_CT = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml"
STAMP = (2026, 11, 17, 0, 0, 0)

ROWS = [["Region", "Sales"], ["North", 120], ["South", 95], ["East", 143], ["West", 88]]


def patt(prst: str, fg: str, bg: str) -> str:
    return (
        f'<a:pattFill prst="{prst}"><a:fgClr><a:srgbClr val="{fg}"/></a:fgClr>'
        f'<a:bgClr><a:srgbClr val="{bg}"/></a:bgClr></a:pattFill>'
    )


def chart_xml(sheet_name: str, series_fill: str, plot_fill: str = "") -> str:
    """A clustered column chart of Sales by Region, values cached."""
    cats = "".join(f'<c:pt idx="{i}"><c:v>{r[0]}</c:v></c:pt>' for i, r in enumerate(ROWS[1:]))
    vals = "".join(f'<c:pt idx="{i}"><c:v>{r[1]}</c:v></c:pt>' for i, r in enumerate(ROWS[1:]))
    n = len(ROWS) - 1
    plot_sppr = f"<c:spPr>{plot_fill}</c:spPr>" if plot_fill else ""
    return (
        HEAD + f'<c:chartSpace xmlns:c="{NS_C}" xmlns:a="{NS_A}" xmlns:r="{NS_R}">'
        '<c:roundedCorners val="0"/><c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/>'
        '<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>'
        '<c:ser><c:idx val="0"/><c:order val="0"/>'
        f"<c:tx><c:strRef><c:f>{sheet_name}!$B$1</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>Sales</c:v></c:pt></c:strCache></c:strRef></c:tx>"
        f'<c:spPr>{series_fill}<a:ln><a:solidFill><a:srgbClr val="404040"/></a:solidFill></a:ln></c:spPr>'
        '<c:invertIfNegative val="0"/>'
        f'<c:cat><c:strRef><c:f>{sheet_name}!$A$2:$A$5</c:f><c:strCache><c:ptCount val="{n}"/>{cats}</c:strCache></c:strRef></c:cat>'
        f'<c:val><c:numRef><c:f>{sheet_name}!$B$2:$B$5</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{n}"/>{vals}</c:numCache></c:numRef></c:val>'
        '</c:ser><c:gapWidth val="80"/><c:axId val="1001"/><c:axId val="1002"/></c:barChart>'
        '<c:catAx><c:axId val="1001"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/>'
        '<c:axPos val="b"/><c:numFmt formatCode="General" sourceLinked="1"/><c:tickLblPos val="nextTo"/><c:crossAx val="1002"/>'
        '<c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/></c:catAx>'
        '<c:valAx><c:axId val="1002"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/>'
        '<c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="1"/><c:tickLblPos val="nextTo"/>'
        '<c:crossAx val="1001"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx>'
        f"{plot_sppr}</c:plotArea><c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>"
    )


def xdr_frame(id_: int, name: str, rid: str, off: tuple[int, int], ext: tuple[int, int]) -> str:
    return (
        f'<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{id_}" name="{name}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>'
        f'<xdr:xfrm><a:off x="{off[0]}" y="{off[1]}"/><a:ext cx="{ext[0]}" cy="{ext[1]}"/></xdr:xfrm>'
        f'<a:graphic><a:graphicData uri="{NS_C}"><c:chart xmlns:c="{NS_C}" xmlns:r="{NS_R}" r:id="{rid}"/></a:graphicData></a:graphic></xdr:graphicFrame>'
    )


def drawing() -> str:
    objs = [
        anchor(3, 1, 6, 7, sp(
            2, "Diagonal box",
            xfrm() + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>' + patt("dkUpDiag", "4472C4", "FFFFFF")
            + '<a:ln w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>',
            body(para("Pattern", '<a:rPr lang="en-US" sz="1400" b="1"/>', '<a:pPr algn="ctr"/>'),
                 '<a:bodyPr rtlCol="0" anchor="ctr"/>'),
        )),
        anchor(7, 1, 10, 7, sp(
            3, "Checker oval",
            xfrm() + '<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>' + patt("smCheck", "C00000", "FFFF00")
            + "<a:ln><a:noFill/></a:ln>",
        )),
    ]
    # The group's child space is its own size (EMU), as Excel writes it.
    gw, gh = 4572000, 2743200
    group = (
        '<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="4" name="Chart group"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>'
        f'<xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{gw}" cy="{gh}"/><a:chOff x="0" y="0"/><a:chExt cx="{gw}" cy="{gh}"/></a:xfrm></xdr:grpSpPr>'
        + '<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="5" name="Caption"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>'
        f'<a:xfrm><a:off x="0" y="0"/><a:ext cx="{gw}" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
        '<a:solidFill><a:srgbClr val="DEEBF7"/></a:solidFill></xdr:spPr>'
        + body(para("Sales by region", '<a:rPr lang="en-US" sz="1200" b="1"/>', '<a:pPr algn="ctr"/>'),
               '<a:bodyPr rtlCol="0" anchor="ctr"/>')
        + "</xdr:sp>"
        + xdr_frame(6, "Grouped chart", "rId1", (0, 457200), (gw, gh - 457200))
        + "</xdr:grpSp>"
    )
    objs.append(anchor(3, 9, 10, 23, group))
    objs.append(anchor(11, 1, 18, 15, xdr_frame(7, "Plain chart", "rId2", (0, 0), (0, 0))))
    return HEAD + f'<xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="{NS_A}">' + "".join(objs) + "</xdr:wsDr>"


def xlsx_parts() -> dict[str, str]:
    over = lambda part, ct: f'<Override PartName="/{part}" ContentType="{ct}"/>'  # noqa: E731
    sml = "application/vnd.openxmlformats-officedocument.spreadsheetml."
    return {
        "[Content_Types].xml": HEAD
        + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        + over("xl/workbook.xml", sml + "sheet.main+xml")
        + over("xl/worksheets/sheet1.xml", sml + "worksheet+xml")
        + over("xl/theme/theme1.xml", "application/vnd.openxmlformats-officedocument.theme+xml")
        + over("xl/styles.xml", sml + "styles+xml")
        + over("xl/drawings/drawing1.xml", "application/vnd.openxmlformats-officedocument.drawing+xml")
        + over("xl/charts/chart1.xml", CHART_CT)
        + over("xl/charts/chart2.xml", CHART_CT)
        + "</Types>",
        "_rels/.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="xl/workbook.xml"/></Relationships>',
        "xl/workbook.xml": HEAD
        + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Patterns" sheetId="1" r:id="rId1"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}">'
        f'<Relationship Id="rId1" Type="{REL}worksheet" Target="worksheets/sheet1.xml"/>'
        f'<Relationship Id="rId3" Type="{REL}theme" Target="theme/theme1.xml"/>'
        f'<Relationship Id="rId4" Type="{REL}styles" Target="styles.xml"/></Relationships>',
        "xl/styles.xml": HEAD
        + f'<styleSheet xmlns="{NS_MAIN}"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
        '<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>'
        '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
        '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        '<cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>'
        '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>',
        "xl/theme/theme1.xml": theme(),
        "xl/worksheets/sheet1.xml": sheet(ROWS, "rId1"),
        "xl/worksheets/_rels/sheet1.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}drawing" Target="../drawings/drawing1.xml"/></Relationships>',
        "xl/drawings/drawing1.xml": drawing(),
        "xl/drawings/_rels/drawing1.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}">'
        f'<Relationship Id="rId1" Type="{REL}chart" Target="../charts/chart1.xml"/>'
        f'<Relationship Id="rId2" Type="{REL}chart" Target="../charts/chart2.xml"/></Relationships>',
        "xl/charts/chart1.xml": chart_xml("Patterns", patt("ltUpDiag", "548235", "FFFFFF")),
        "xl/charts/chart2.xml": chart_xml(
            "Patterns", '<a:solidFill><a:srgbClr val="ED7D31"/></a:solidFill>', patt("pct20", "7F7F7F", "FFFFFF")
        ),
    }


# --- docx -------------------------------------------------------------------

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
WPS = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
WPG = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"
MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"
DOC_NS = (
    f'xmlns:w="{W}" xmlns:r="{NS_R}" xmlns:wp="{WP}" xmlns:a="{NS_A}" xmlns:wps="{WPS}" '
    f'xmlns:wpg="{WPG}" xmlns:c="{NS_C}" xmlns:mc="{MC}" '
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" mc:Ignorable="w14"'
)
EMU = 9525


def wp_anchor(doc_id: int, name: str, x: int, y: int, cx: int, cy: int, uri: str, content: str) -> str:
    return (
        f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" '
        f'relativeHeight="{251659264 + doc_id}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1">'
        f'<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="margin"><wp:posOffset>{x * EMU}</wp:posOffset></wp:positionH>'
        f'<wp:positionV relativeFrom="paragraph"><wp:posOffset>{y * EMU}</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{cx * EMU}" cy="{cy * EMU}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>'
        f'<wp:wrapTopAndBottom/><wp:docPr id="{doc_id}" name="{name}"/><wp:cNvGraphicFramePr/>'
        f'<a:graphic><a:graphicData uri="{uri}">{content}</a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>'
    )


def wps(prst: str, fill: str, cx: int, cy: int, off: tuple[int, int] = (0, 0), text: str = "", tag: str = "wps:wsp") -> str:
    txbx = (
        f'<wps:txbx><w:txbxContent><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>{text}</w:t></w:r></w:p></w:txbxContent></wps:txbx>'
        if text
        else ""
    )
    return (
        f"<{tag}><wps:cNvSpPr/><wps:spPr><a:xfrm><a:off x=\"{off[0]}\" y=\"{off[1]}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>"
        f'<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom>{fill}'
        '<a:ln w="9525"><a:solidFill><a:srgbClr val="404040"/></a:solidFill></a:ln></wps:spPr>'
        f'{txbx}<wps:bodyPr rot="0" vert="horz" wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" anchor="ctr"/></{tag}>'
    )


def docx_document() -> str:
    rect = wp_anchor(
        1, "Pattern rect", 0, 10, 240, 120, WPS,
        wps("rect", patt("wdDnDiag", "548235", "FFF2CC"), 240 * EMU, 120 * EMU),
    )
    gw, gh = 400 * EMU, 260 * EMU
    cap = 40 * EMU
    group = (
        "<wpg:wgp><wpg:cNvGrpSpPr/><wpg:grpSpPr>"
        f'<a:xfrm><a:off x="0" y="0"/><a:ext cx="{gw}" cy="{gh}"/><a:chOff x="0" y="0"/><a:chExt cx="{gw}" cy="{gh}"/></a:xfrm></wpg:grpSpPr>'
        + wps("rect", '<a:solidFill><a:srgbClr val="DEEBF7"/></a:solidFill>', gw, cap, text="Group caption")
        + '<wpg:graphicFrame><wpg:cNvPr id="12" name="Group chart"/><wpg:cNvFrPr/>'
        f'<wpg:xfrm><a:off x="0" y="{cap}"/><a:ext cx="{gw}" cy="{gh - cap}"/></wpg:xfrm>'
        f'<a:graphic><a:graphicData uri="{NS_C}"><c:chart r:id="rIdChart1"/></a:graphicData></a:graphic></wpg:graphicFrame>'
        "</wpg:wgp>"
    )
    grp = wp_anchor(2, "Chart group", 0, 150, 400, 260, WPG, group)
    p = lambda t, runs="": f"<w:p><w:r><w:t>{t}</w:t></w:r>{runs}</w:p>"  # noqa: E731
    sect = (
        '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" '
        'w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>'
    )
    return (
        HEAD + f"<w:document {DOC_NS}><w:body>"
        + p("A shape with a pattern fill, and a group holding a chart.", rect + grp)
        + p("After the drawings.")
        + sect + "</w:body></w:document>"
    )


def docx_parts() -> dict[str, str]:
    return {
        "[Content_Types].xml": HEAD
        + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
        f'<Override PartName="/word/charts/chart1.xml" ContentType="{CHART_CT}"/>'
        "</Types>",
        "_rels/.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="word/document.xml"/></Relationships>',
        "word/document.xml": docx_document(),
        "word/_rels/document.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}">'
        f'<Relationship Id="rIdStyles" Type="{REL}styles" Target="styles.xml"/>'
        f'<Relationship Id="rIdChart1" Type="{REL}chart" Target="charts/chart1.xml"/></Relationships>',
        "word/styles.xml": HEAD
        + f'<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/>'
        '<w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>'
        '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>',
        "word/charts/chart1.xml": chart_xml("Sheet1", patt("dkDnDiag", "C55A11", "FFFFFF")),
    }


def zipped(parts: dict[str, str]) -> bytes:
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts.items():
            info = zipfile.ZipInfo(name, STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, text.encode("utf-8"))
    return out.getvalue()


FILES = {XLSX: xlsx_parts, DOCX: docx_parts}


def soffice(lo: str) -> int:
    """The .xlsx converted to .ods and .xls by LibreOffice."""
    import shutil
    import subprocess
    import tempfile

    out_dir = os.path.join(ROOT, "tests", "corpus", "xls")
    profile = os.path.expanduser("~/.cache/ktmp/lo-gen-patterns")
    with tempfile.TemporaryDirectory(dir=os.path.expanduser("~/.cache/ktmp")) as tmp:
        src = os.path.join(tmp, "patterns_groups.xlsx")
        shutil.copy(XLSX, src)
        for fmt in ("ods", "xls"):
            made = os.path.join(tmp, f"patterns_groups.{fmt}")
            # A new profile makes LibreOffice restart once (exit 81).
            for _ in range(3):
                subprocess.run(
                    [lo, "--headless", f"-env:UserInstallation=file://{profile}", "--convert-to", fmt, "--outdir", tmp, src],
                    capture_output=True,
                )
                if os.path.exists(made):
                    break
            else:
                raise SystemExit(f"LibreOffice didn't write {made}")
            shutil.copy(os.path.join(tmp, f"patterns_groups.{fmt}"), os.path.join(out_dir, f"patterns_groups.{fmt}"))
            print(os.path.join(out_dir, f"patterns_groups.{fmt}"))
    return 0


def main() -> int:
    if "--soffice" in sys.argv:
        i = sys.argv.index("--soffice")
        lo = sys.argv[i + 1] if len(sys.argv) > i + 1 else os.path.expanduser("~/.cache/lo/lo.AppImage")
        return soffice(lo)
    bad = 0
    for path, make in FILES.items():
        parts = make()
        if "--check" in sys.argv:
            try:
                with zipfile.ZipFile(path) as z:
                    have = {n: z.read(n).decode("utf-8") for n in z.namelist()}
            except FileNotFoundError:
                have = None
            if have != parts:
                print(f"{path} is out of date: run {sys.argv[0]}", file=sys.stderr)
                bad += 1
        else:
            with open(path, "wb") as f:
                f.write(zipped(parts))
            print(path)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
