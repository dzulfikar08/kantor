#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/23_shapes.xlsx: shapes, text boxes,
connectors, a group and a form control's shape on a worksheet.

openpyxl can't write shapes and XlsxWriter only writes a few, so the
package is raw SpreadsheetML written here (standard library only), with
the drawing XML by hand, as Excel writes it. Sheet "Shapes" (A1:B4 of
data) holds, by name:

* "Revenue box": a rectangle, solid blue fill, dark blue 2 pt outline,
  "Revenue" bold 14 pt white, centred both ways.
* "Gradient oval": an ellipse with a linear gradient, no outline, "Gradient".
* "Dashed frame": a rounded rectangle, no fill, a red dashed outline, two
  paragraphs (left "First line", right italic "Second line"), wide insets.
* "Arrow line": a connector with an oval head and a triangle tail.
* "Turned arrow": a right arrow turned 30 degrees, filled by the theme's
  style (fillRef 1, accent 2).
* "Notes box": a text box as Excel's Insert > Text Box makes it, two
  lines, shrunk to 90 % (normAutofit).
* "Pair": a group of a square and a circle (child coordinates).
* "Button 1": a form control's shape inside mc:AlternateContent.
* "Note callout": a rectangular callout, "Note!".
* "Hidden shape": hidden (cNvPr hidden="1"), never shown.

Sheet "Other" has no drawing. CC0-1.0 like the rest of the corpus.
Deterministic (fixed zip timestamps): `--check` compares parts.

    python3 scripts/corpus/gen_shapes_xlsx.py           # write
    python3 scripts/corpus/gen_shapes_xlsx.py --check   # CI
"""

from __future__ import annotations

import os
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "tests", "corpus", "xlsx", "23_shapes.xlsx")

NS_MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS_PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
NS_XDR = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"
NS_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
NS_MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"
NS_A14 = "http://schemas.microsoft.com/office/drawing/2010/main"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
HEAD = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'


def anchor(c0: int, r0: int, c1: int, r1: int, body: str, off: tuple[int, int] = (0, 0)) -> str:
    """A two-cell anchor from (c0, r0) to (c1, r1), 0-based cells."""
    return (
        "<xdr:twoCellAnchor>"
        f"<xdr:from><xdr:col>{c0}</xdr:col><xdr:colOff>{off[0]}</xdr:colOff><xdr:row>{r0}</xdr:row><xdr:rowOff>{off[1]}</xdr:rowOff></xdr:from>"
        f"<xdr:to><xdr:col>{c1}</xdr:col><xdr:colOff>{off[0]}</xdr:colOff><xdr:row>{r1}</xdr:row><xdr:rowOff>{off[1]}</xdr:rowOff></xdr:to>"
        f"{body}<xdr:clientData/></xdr:twoCellAnchor>"
    )


def xfrm(rot: int = 0, cx: int = 1828800, cy: int = 1143000) -> str:
    r = f' rot="{rot}"' if rot else ""
    return f'<a:xfrm{r}><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'


def sp(id_: int, name: str, sppr: str, txbody: str = "", style: str = "", tx_box: bool = False, hidden: bool = False) -> str:
    h = ' hidden="1"' if hidden else ""
    tb = ' txBox="1"' if tx_box else ""
    return (
        f'<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{id_}" name="{name}"{h}/><xdr:cNvSpPr{tb}/></xdr:nvSpPr>'
        f"<xdr:spPr>{sppr}</xdr:spPr>{style}{txbody}</xdr:sp>"
    )


def para(text: str, rpr: str = '<a:rPr lang="en-US" sz="1100"/>', ppr: str = "") -> str:
    return f"<a:p>{ppr}<a:r>{rpr}<a:t>{text}</a:t></a:r></a:p>"


def body(paras: str, bodypr: str = '<a:bodyPr rtlCol="0" anchor="t"/>') -> str:
    return f"<xdr:txBody>{bodypr}<a:lstStyle/>{paras}</xdr:txBody>"


SHAPE_STYLE = (
    '<xdr:style><a:lnRef idx="2"><a:schemeClr val="accent2"><a:shade val="50000"/></a:schemeClr></a:lnRef>'
    '<a:fillRef idx="1"><a:schemeClr val="accent2"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent2"/></a:effectRef>'
    '<a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></xdr:style>'
)
TEXTBOX_STYLE = (
    '<xdr:style><a:lnRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef>'
    '<a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="dk1"/></a:fontRef></xdr:style>'
)


def drawing() -> str:
    objs = []
    objs.append(anchor(1, 1, 4, 7, sp(
        2, "Revenue box",
        xfrm() + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="4472C4"/></a:solidFill>'
        '<a:ln w="25400"><a:solidFill><a:srgbClr val="1F3864"/></a:solidFill></a:ln>',
        body(para("Revenue", '<a:rPr lang="en-US" sz="1400" b="1"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:rPr>', '<a:pPr algn="ctr"/>'),
             '<a:bodyPr rtlCol="0" anchor="ctr"/>'),
    )))
    objs.append(anchor(6, 1, 9, 7, sp(
        3, "Gradient oval",
        xfrm() + '<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>'
        '<a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:srgbClr val="FFC000"/></a:gs><a:gs pos="100000"><a:srgbClr val="ED7D31"/></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill>'
        "<a:ln><a:noFill/></a:ln>",
        body(para("Gradient", ppr='<a:pPr algn="ctr"/>'), '<a:bodyPr rtlCol="0" anchor="ctr"/>'),
    )))
    objs.append(anchor(1, 9, 4, 13, sp(
        4, "Dashed frame",
        xfrm() + '<a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom><a:noFill/>'
        '<a:ln w="19050"><a:solidFill><a:srgbClr val="C00000"/></a:solidFill><a:prstDash val="dash"/></a:ln>',
        body(para("First line", ppr='<a:pPr algn="l"/>')
             + para("Second line", '<a:rPr lang="en-US" sz="1100" i="1"/>', '<a:pPr algn="r"/>'),
             '<a:bodyPr lIns="182880" rIns="182880" rtlCol="0" anchor="t"/>'),
    )))
    objs.append(anchor(6, 9, 9, 13,
        '<xdr:cxnSp macro=""><xdr:nvCxnSpPr><xdr:cNvPr id="5" name="Arrow line"/><xdr:cNvCxnSpPr/></xdr:nvCxnSpPr>'
        f'<xdr:spPr>{xfrm()}<a:prstGeom prst="line"><a:avLst/></a:prstGeom>'
        '<a:ln w="28575"><a:solidFill><a:srgbClr val="70AD47"/></a:solidFill><a:headEnd type="oval"/><a:tailEnd type="triangle"/></a:ln></xdr:spPr>'
        '<xdr:style><a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="0"><a:schemeClr val="accent1"/></a:fillRef>'
        '<a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="tx1"/></a:fontRef></xdr:style></xdr:cxnSp>'))
    objs.append(anchor(11, 1, 14, 6, sp(
        6, "Turned arrow", xfrm(rot=1800000) + '<a:prstGeom prst="rightArrow"><a:avLst/></a:prstGeom>', style=SHAPE_STYLE,
    )))
    objs.append(anchor(11, 8, 15, 12, sp(
        7, "Notes box",
        xfrm() + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="lt1"/></a:solidFill>'
        '<a:ln w="9525" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"><a:shade val="50000"/></a:schemeClr></a:solidFill></a:ln>',
        body(para("A text box") + para("with two lines"),
             '<a:bodyPr vertOverflow="clip" horzOverflow="clip" wrap="square" rtlCol="0" anchor="t"><a:normAutofit fontScale="90000"/></a:bodyPr>'),
        TEXTBOX_STYLE, tx_box=True,
    )))
    group = (
        '<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="8" name="Pair"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr>'
        '<xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/><a:chOff x="0" y="0"/><a:chExt cx="2000" cy="1000"/></a:xfrm></xdr:grpSpPr>'
        + '<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="9" name="Square"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>'
        '<a:xfrm><a:off x="0" y="0"/><a:ext cx="900" cy="1000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
        '<a:solidFill><a:srgbClr val="5B9BD5"/></a:solidFill></xdr:spPr></xdr:sp>'
        + '<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="10" name="Circle"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>'
        '<a:xfrm><a:off x="1100" y="0"/><a:ext cx="900" cy="1000"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>'
        '<a:solidFill><a:srgbClr val="A5A5A5"/></a:solidFill></xdr:spPr></xdr:sp>'
        "</xdr:grpSp>"
    )
    objs.append(anchor(1, 15, 5, 21, group))
    button = anchor(6, 15, 8, 17, sp(
        1025, "Button 1",
        xfrm() + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="9525"><a:miter lim="800000"/></a:ln>',
        body(para("Run", '<a:rPr lang="en-US" sz="1100"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:rPr>', '<a:pPr algn="ctr"/>'),
             '<a:bodyPr vertOverflow="clip" wrap="square" lIns="27432" tIns="27432" rIns="27432" bIns="27432" anchor="ctr" upright="1"/>'),
    ))
    objs.append(
        f'<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:a14="{NS_A14}" Requires="a14">{button}</mc:Choice><mc:Fallback/></mc:AlternateContent>'
    )
    objs.append(anchor(9, 15, 12, 19, sp(
        11, "Note callout",
        xfrm() + '<a:prstGeom prst="wedgeRectCallout"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FFF2CC"/></a:solidFill>'
        '<a:ln w="12700"><a:solidFill><a:srgbClr val="BF9000"/></a:solidFill></a:ln>',
        body(para("Note!", '<a:rPr lang="en-US" sz="1100"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:rPr>')),
    )))
    objs.append(anchor(13, 15, 15, 18, sp(
        12, "Hidden shape", xfrm() + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>',
        hidden=True,
    )))
    return HEAD + f'<xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="{NS_A}">' + "".join(objs) + "</xdr:wsDr>"


def sheet(rows: list[list], drawing_rid: str | None) -> str:
    def cell(r: int, c: int, v) -> str:
        ref = f"{chr(65 + c)}{r + 1}"
        if isinstance(v, (int, float)):
            return f'<c r="{ref}"><v>{v}</v></c>'
        return f'<c r="{ref}" t="inlineStr"><is><t>{v}</t></is></c>'

    data = "".join(
        f'<row r="{r + 1}">' + "".join(cell(r, c, v) for c, v in enumerate(row)) + "</row>"
        for r, row in enumerate(rows)
    )
    d = f'<drawing r:id="{drawing_rid}"/>' if drawing_rid else ""
    return HEAD + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData>{data}</sheetData>{d}</worksheet>'


def theme() -> str:
    """Office's theme colours, Calibri, and a format scheme (the styles
    `fillRef` / `lnRef` point at)."""
    clr = "".join(
        f'<a:{n}><a:srgbClr val="{v}"/></a:{n}>'
        for n, v in [("dk2", "44546A"), ("lt2", "E7E6E6"), ("accent1", "4472C4"), ("accent2", "ED7D31"),
                     ("accent3", "A5A5A5"), ("accent4", "FFC000"), ("accent5", "5B9BD5"), ("accent6", "70AD47"),
                     ("hlink", "0563C1"), ("folHlink", "954F72")]
    )
    fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    line = "".join(
        f'<a:ln w="{w}" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
        '<a:prstDash val="solid"/><a:miter lim="800000"/></a:ln>'
        for w in (6350, 12700, 19050)
    )
    effect = "<a:effectStyle><a:effectLst/></a:effectStyle>"
    return (
        HEAD + f'<a:theme xmlns:a="{NS_A}" name="Office Theme"><a:themeElements>'
        '<a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>'
        f'<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>{clr}</a:clrScheme>'
        '<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>'
        '<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>'
        f'<a:fmtScheme name="Office"><a:fillStyleLst>{fill * 3}</a:fillStyleLst><a:lnStyleLst>{line}</a:lnStyleLst>'
        f'<a:effectStyleLst>{effect * 3}</a:effectStyleLst><a:bgFillStyleLst>{fill * 3}</a:bgFillStyleLst></a:fmtScheme>'
        "</a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"
    )


def parts() -> dict[str, str]:
    return {
        "[Content_Types].xml": HEAD
        + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        '<Override PartName="/xl/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
        '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>'
        '<Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/>'
        "</Types>",
        "_rels/.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="xl/workbook.xml"/></Relationships>',
        "xl/workbook.xml": HEAD
        + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets>'
        '<sheet name="Shapes" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId2"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}">'
        f'<Relationship Id="rId1" Type="{REL}worksheet" Target="worksheets/sheet1.xml"/>'
        f'<Relationship Id="rId2" Type="{REL}worksheet" Target="worksheets/sheet2.xml"/>'
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
        "xl/worksheets/sheet1.xml": sheet([["Item", "Amount"], ["North", 120], ["South", 95], ["East", 143]], "rId1"),
        "xl/worksheets/_rels/sheet1.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}drawing" Target="../drawings/drawing1.xml"/></Relationships>',
        "xl/worksheets/sheet2.xml": sheet([["Nothing drawn here"]], None),
        "xl/drawings/drawing1.xml": drawing(),
    }


def write(path: str) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts().items():
            info = zipfile.ZipInfo(name, (2026, 11, 15, 9, 0, 0))
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
