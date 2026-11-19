#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/29_custom_theme.xlsx: a workbook whose theme is
not Office's, to check that a save keeps the file's own theme.

Raw SpreadsheetML (standard library only). The theme "Kantor Plum":

* colours: plum, teal, ochre… accents (none of Office's),
* fonts: Georgia headings, Verdana body,
* line styles 0.75 / 1.75 / 2.5 pt (Office has 0.5 / 1 / 1.5 pt,
  IronCalc's own theme 1 / 1.5 / 2 pt),
* a background fill style with a picture (so the theme part has its own
  relationships, to `xl/media/image1.png`),
* `objectDefaults`, an `extraClrSchemeLst` entry and an `extLst`.

Sheet "Themed": cells in the body font (`scheme minor`), the heading in the
heading font (`scheme major`); fills and font colours that are theme colours
with tints; a picture outlined only by its style (`a:lnRef idx="2"`, the
theme's 1.75 pt line in accent 2) and a column chart whose series colour is
`accent1`. Sheet "Other" is empty. CC0-1.0 like the rest of the corpus.
Deterministic (fixed zip timestamps): `--check` compares parts.

    python3 scripts/corpus/gen_custom_theme_xlsx.py           # write
    python3 scripts/corpus/gen_custom_theme_xlsx.py --check   # CI
"""

from __future__ import annotations

import os
import sys
import zipfile

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_shapes_xlsx as base  # noqa: E402  (sheet and anchor writers)
import gen_shaped_pictures_xlsx as pics  # noqa: E402  (PNG, picture writers)

OUT = os.path.join(base.ROOT, "tests", "corpus", "xlsx", "29_custom_theme.xlsx")
HEAD = base.HEAD
REL = base.REL
NS_PKG = base.NS_PKG
NS_MAIN = base.NS_MAIN
NS_R = base.NS_R
NS_A = base.NS_A
NS_C = "http://schemas.openxmlformats.org/drawingml/2006/chart"

ACCENTS = [("dk2", "2E1A47"), ("lt2", "F3EEE6"), ("accent1", "8E44AD"), ("accent2", "16A085"),
           ("accent3", "D68910"), ("accent4", "C0392B"), ("accent5", "2471A3"), ("accent6", "7D8C2E"),
           ("hlink", "6C3483"), ("folHlink", "1F618D")]
LINE_WIDTHS = (9525, 22225, 31750)


def theme() -> str:
    clr = "".join(f'<a:{n}><a:srgbClr val="{v}"/></a:{n}>' for n, v in ACCENTS)
    fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    grad = (
        '<a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="80000"/></a:schemeClr></a:gs>'
        '<a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="70000"/></a:schemeClr></a:gs></a:gsLst>'
        '<a:lin ang="5400000" scaled="0"/></a:gradFill>'
    )
    picture = (
        f'<a:blipFill rotWithShape="1"><a:blip xmlns:r="{NS_R}" r:embed="rId1"/>'
        '<a:tile tx="0" ty="0" sx="100000" sy="100000" flip="none" algn="tl"/></a:blipFill>'
    )
    line = "".join(
        f'<a:ln w="{w}" cap="rnd" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
        '<a:prstDash val="solid"/><a:round/></a:ln>'
        for w in LINE_WIDTHS
    )
    shadow = (
        '<a:effectStyle><a:effectLst><a:outerShdw blurRad="76200" dist="38100" dir="2700000" algn="tl" rotWithShape="0">'
        '<a:srgbClr val="2E1A47"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst></a:effectStyle>'
    )
    effect = "<a:effectStyle><a:effectLst/></a:effectStyle>"
    extra = (
        '<a:extraClrScheme><a:clrScheme name="Kantor Plum Dusk"><a:dk1><a:srgbClr val="101010"/></a:dk1>'
        '<a:lt1><a:srgbClr val="FAFAFA"/></a:lt1>'
        + "".join(f'<a:{n}><a:srgbClr val="{v}"/></a:{n}>' for (n, _), (_, v) in zip(ACCENTS, ACCENTS[1:] + ACCENTS[:1]))
        + '</a:clrScheme><a:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" '
        'accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>'
        "</a:extraClrScheme>"
    )
    return (
        HEAD + f'<a:theme xmlns:a="{NS_A}" name="Kantor Plum"><a:themeElements>'
        '<a:clrScheme name="Kantor Plum"><a:dk1><a:srgbClr val="1B1B1B"/></a:dk1>'
        f'<a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>{clr}</a:clrScheme>'
        '<a:fontScheme name="Kantor Plum"><a:majorFont><a:latin typeface="Georgia"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>'
        '<a:minorFont><a:latin typeface="Verdana"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>'
        f'<a:fmtScheme name="Kantor Plum"><a:fillStyleLst>{fill}{grad}{grad}</a:fillStyleLst><a:lnStyleLst>{line}</a:lnStyleLst>'
        f'<a:effectStyleLst>{effect}{effect}{shadow}</a:effectStyleLst><a:bgFillStyleLst>{fill}{grad}{picture}</a:bgFillStyleLst></a:fmtScheme>'
        "</a:themeElements>"
        '<a:objectDefaults><a:spDef><a:spPr/><a:bodyPr/><a:lstStyle/><a:style><a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef>'
        '<a:fillRef idx="2"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef>'
        '<a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></a:style></a:spDef></a:objectDefaults>'
        f"<a:extraClrSchemeLst>{extra}</a:extraClrSchemeLst>"
        '<a:extLst><a:ext uri="{05A4C25C-085E-4340-85A3-A5531E510DB2}">'
        '<thm15:themeFamily xmlns:thm15="http://schemas.microsoft.com/office/thememl/2012/main" name="Kantor Plum" '
        'id="{0B1D2C3E-4F50-4617-8A29-3B4C5D6E7F80}" vid="{9A8B7C6D-5E4F-4A3B-9C2D-1E0F1A2B3C4D}"/></a:ext></a:extLst>'
        "</a:theme>"
    )


def styles() -> str:
    fonts = (
        '<fonts count="3">'
        '<font><sz val="10"/><color theme="1"/><name val="Verdana"/><family val="2"/><scheme val="minor"/></font>'
        '<font><b/><sz val="14"/><color theme="4" tint="-0.249977111117893"/><name val="Georgia"/><family val="1"/><scheme val="major"/></font>'
        '<font><sz val="10"/><color theme="5"/><name val="Verdana"/><family val="2"/><scheme val="minor"/></font>'
        "</fonts>"
    )
    fills = (
        '<fills count="4"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>'
        '<fill><patternFill patternType="solid"><fgColor theme="4" tint="0.79998168889431442"/><bgColor indexed="64"/></patternFill></fill>'
        '<fill><patternFill patternType="solid"><fgColor theme="6" tint="0.39997558519241921"/><bgColor indexed="64"/></patternFill></fill>'
        "</fills>"
    )
    borders = (
        '<borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border>'
        '<border><left/><right/><top/><bottom style="medium"><color theme="5"/></bottom><diagonal/></border></borders>'
    )
    xfs = (
        '<cellXfs count="4"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
        '<xf numFmtId="0" fontId="1" fillId="0" borderId="1" xfId="0" applyFont="1" applyBorder="1"/>'
        '<xf numFmtId="0" fontId="0" fillId="2" borderId="0" xfId="0" applyFill="1"/>'
        '<xf numFmtId="0" fontId="2" fillId="3" borderId="0" xfId="0" applyFont="1" applyFill="1"/></cellXfs>'
    )
    return (
        HEAD + f'<styleSheet xmlns="{NS_MAIN}">{fonts}{fills}{borders}'
        '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        f'{xfs}<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>'
    )


def sheet() -> str:
    rows = [
        ('<c r="A1" s="1" t="inlineStr"><is><t>Quarterly sales</t></is></c>',),
        ('<c r="A2" t="inlineStr"><is><t>Quarter</t></is></c>', '<c r="B2" t="inlineStr"><is><t>Amount</t></is></c>'),
        ('<c r="A3" s="2" t="inlineStr"><is><t>Q1</t></is></c>', '<c r="B3" s="2"><v>120</v></c>'),
        ('<c r="A4" s="3" t="inlineStr"><is><t>Q2</t></is></c>', '<c r="B4" s="3"><v>95</v></c>'),
        ('<c r="A5" t="inlineStr"><is><t>Q3</t></is></c>', '<c r="B5"><v>143</v></c>'),
    ]
    data = "".join(f'<row r="{i + 1}">' + "".join(r) + "</row>" for i, r in enumerate(rows))
    return (
        HEAD + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData>{data}</sheetData>'
        '<drawing r:id="rId1"/></worksheet>'
    )


def chart() -> str:
    def ref(f: str, kind: str, vals: list) -> str:
        pts = "".join(f'<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>' for i, v in enumerate(vals))
        cache = kind[:3] + "Cache"
        return f"<c:{kind}><c:f>{f}</c:f><c:{cache}>" + (
            "<c:formatCode>General</c:formatCode>" if kind == "numRef" else ""
        ) + f'<c:ptCount val="{len(vals)}"/>{pts}</c:{cache}></c:{kind}>'

    return (
        HEAD + f'<c:chartSpace xmlns:c="{NS_C}" xmlns:a="{NS_A}" xmlns:r="{NS_R}">'
        '<c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/>'
        '<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>'
        '<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Amount</c:v></c:tx>'
        '<c:spPr><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></c:spPr>'
        f'<c:cat>{ref("Themed!$A$3:$A$5", "strRef", ["Q1", "Q2", "Q3"])}</c:cat>'
        f'<c:val>{ref("Themed!$B$3:$B$5", "numRef", [120, 95, 143])}</c:val></c:ser>'
        '<c:axId val="10"/><c:axId val="20"/></c:barChart>'
        '<c:catAx><c:axId val="10"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/>'
        '<c:axPos val="b"/><c:crossAx val="20"/></c:catAx>'
        '<c:valAx><c:axId val="20"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/>'
        '<c:axPos val="l"/><c:crossAx val="10"/></c:valAx>'
        '</c:plotArea><c:plotVisOnly val="1"/></c:chart></c:chartSpace>'
    )


def drawing() -> str:
    picture = pics.pic(
        2, "Styled photo", pics.prst("rect"),
        style=pics.style(2, '<a:schemeClr val="accent2"/>'),
    )
    frame = (
        '<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="3" name="Sales chart"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>'
        '<xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>'
        f'<a:graphic><a:graphicData uri="{NS_C}"><c:chart xmlns:c="{NS_C}" xmlns:r="{NS_R}" r:id="rId2"/></a:graphicData></a:graphic>'
        "</xdr:graphicFrame>"
    )
    return (
        HEAD + f'<xdr:wsDr xmlns:xdr="{base.NS_XDR}" xmlns:a="{NS_A}">'
        + base.anchor(3, 1, 6, 9, picture)
        + base.anchor(7, 1, 14, 16, frame)
        + "</xdr:wsDr>"
    )


def parts() -> dict[str, bytes]:
    ct = "application/vnd.openxmlformats-officedocument."
    rels = lambda body: HEAD + f'<Relationships xmlns="{NS_PKG}">{body}</Relationships>'  # noqa: E731
    out = {
        "[Content_Types].xml": HEAD
        + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>'
        f'<Override PartName="/xl/workbook.xml" ContentType="{ct}spreadsheetml.sheet.main+xml"/>'
        f'<Override PartName="/xl/worksheets/sheet1.xml" ContentType="{ct}spreadsheetml.worksheet+xml"/>'
        f'<Override PartName="/xl/worksheets/sheet2.xml" ContentType="{ct}spreadsheetml.worksheet+xml"/>'
        f'<Override PartName="/xl/theme/theme1.xml" ContentType="{ct}theme+xml"/>'
        f'<Override PartName="/xl/styles.xml" ContentType="{ct}spreadsheetml.styles+xml"/>'
        f'<Override PartName="/xl/drawings/drawing1.xml" ContentType="{ct}drawing+xml"/>'
        f'<Override PartName="/xl/charts/chart1.xml" ContentType="{ct}drawingml.chart+xml"/>'
        "</Types>",
        "_rels/.rels": rels(f'<Relationship Id="rId1" Type="{REL}officeDocument" Target="xl/workbook.xml"/>'),
        "xl/workbook.xml": HEAD
        + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets>'
        '<sheet name="Themed" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId2"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": rels(
            f'<Relationship Id="rId1" Type="{REL}worksheet" Target="worksheets/sheet1.xml"/>'
            f'<Relationship Id="rId2" Type="{REL}worksheet" Target="worksheets/sheet2.xml"/>'
            f'<Relationship Id="rId3" Type="{REL}theme" Target="theme/theme1.xml"/>'
            f'<Relationship Id="rId4" Type="{REL}styles" Target="styles.xml"/>'
        ),
        "xl/styles.xml": styles(),
        "xl/theme/theme1.xml": theme(),
        "xl/theme/_rels/theme1.xml.rels": rels(f'<Relationship Id="rId1" Type="{REL}image" Target="../media/image1.png"/>'),
        "xl/worksheets/sheet1.xml": sheet(),
        "xl/worksheets/_rels/sheet1.xml.rels": rels(
            f'<Relationship Id="rId1" Type="{REL}drawing" Target="../drawings/drawing1.xml"/>'
        ),
        "xl/worksheets/sheet2.xml": base.sheet([["Nothing here"]], None),
        "xl/drawings/drawing1.xml": drawing(),
        "xl/drawings/_rels/drawing1.xml.rels": rels(
            f'<Relationship Id="rId1" Type="{REL}image" Target="../media/image2.png"/>'
            f'<Relationship Id="rId2" Type="{REL}chart" Target="../charts/chart1.xml"/>'
        ),
        "xl/charts/chart1.xml": chart(),
    }
    enc = {n: t.encode("utf-8") for n, t in out.items()}
    enc["xl/media/image1.png"] = pics.png(8, 8)
    enc["xl/media/image2.png"] = pics.png()
    return enc


def write(path: str) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts().items():
            info = zipfile.ZipInfo(name, (2026, 11, 19, 9, 0, 0))
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
