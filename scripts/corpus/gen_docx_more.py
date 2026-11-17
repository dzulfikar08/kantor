#!/usr/bin/env python3
"""Write more Docs fixtures (CC0, synthetic) with the standard library only,
as raw WordprocessingML (deterministic: same bytes every run):

* textboxes.docx: a text box holding a paragraph, an inline picture and a
  2 × 2 table (Word's `mc:AlternateContent` with a VML fallback), set to
  "Resize shape to fit text" (`a:spAutoFit`) but saved shorter than its
  text; a plain text box that also fits its text; body text around them.
* formulas.docx: fields Word computes: `=` formulas with bookmarks and
  table cell references (`SUM(ABOVE)`, `B2*C2`, `AVERAGE(LEFT)`), `IF`,
  `COMPARE`, `STYLEREF`, `LISTNUM`, `SEQ` with `\\r`/`\\c`/`\\h`, `SET`/`REF`,
  a simple `INDEX`, and tables of contents limited to a bookmark (`\\b`)
  and built from `TC` entries (`\\f`). Every result is saved wrong on
  purpose ("0"), so tests see Kantor work it out.
* styleref_index.docx: a header with `STYLEREF "Heading 1"` over four
  pages (two chapters, a page without a heading, the index page), and an
  `INDEX \\h "A"` of `XE` entries with sub-entries and a cross-reference
  (`\\t "See Apples"`). Results saved wrong ("0").

Regenerate with `python3 scripts/corpus/gen_docx_more.py tests/corpus/docx`;
`--check` compares with the files in that folder instead.
"""
import io
import struct
import sys
import zipfile
import zlib
from pathlib import Path

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
NS = (
    f'xmlns:w="{W}" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
    'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
    'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" '
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" '
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
    'xmlns:v="urn:schemas-microsoft-com:vml" '
    'xmlns:o="urn:schemas-microsoft-com:office:office" '
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
    'mc:Ignorable="w14"'
)
EMU = 9525  # per px
STAMP = (2026, 11, 17, 0, 0, 0)


def png(w, h, rgb):
    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    row = b"\x00" + bytes(rgb) * w
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(row * h, 9)) + chunk(b"IEND", b"")


STYLES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="TOC1"><w:name w:val="toc 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:after="100"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="TOC2"><w:name w:val="toc 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:after="100"/><w:ind w:left="220"/></w:pPr></w:style>
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>
</w:styles>"""

SECT = (
    '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" '
    'w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>'
)


def package(body, media=(), numbering=None, header=None):
    """A .docx's bytes: `body` inside w:body, `media` = [(rId, name, bytes)]."""
    rels = [
        '<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
    ]
    for rid, name, _ in media:
        rels.append(
            f'<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{name}"/>'
        )
    overrides = (
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
    )
    sect = SECT
    if header:
        rels.append(
            '<Relationship Id="rIdHeader1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>'
        )
        sect = SECT.replace("<w:sectPr>", '<w:sectPr><w:headerReference w:type="default" r:id="rIdHeader1"/>')
    if numbering:
        rels.append(
            '<Relationship Id="rIdNumbering" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>'
        )
        overrides += '<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>'
    files = {
        "[Content_Types].xml": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Default Extension="png" ContentType="image/png"/>' + overrides + "</Types>",
        "_rels/.rels": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
        "</Relationships>",
        "word/document.xml": f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}{sect}</w:body></w:document>',
        "word/_rels/document.xml.rels": '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + "".join(rels) + "</Relationships>",
        "word/styles.xml": STYLES,
    }
    if numbering:
        files["word/numbering.xml"] = numbering
    if header:
        files["word/header1.xml"] = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr {NS}>{header}</w:hdr>'
        files["[Content_Types].xml"] = files["[Content_Types].xml"].replace(
            "</Types>",
            '<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/></Types>',
        )
    for _, name, data in media:
        files[f"word/media/{name}"] = data
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            info = zipfile.ZipInfo(name, STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data.encode() if isinstance(data, str) else data)
    return out.getvalue()


def esc(t):
    return t.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def para(text, style=None, extra=""):
    ppr = f'<w:pPr><w:pStyle w:val="{style}"/></w:pPr>' if style else ""
    run = f'<w:r><w:t xml:space="preserve">{esc(text)}</w:t></w:r>' if text else ""
    return f"<w:p>{ppr}{run}{extra}</w:p>"


# ── textboxes.docx ─────────────────────────────────────────────────────────

LOREM = (
    "Kantor edits text boxes where they're drawn, as Word does: pictures and tables inside a "
    "box are shown and edited in place, and a box set to resize to fit its text grows with it. "
)


def inline_picture(rid, cx, cy, doc_id, name):
    return (
        f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>'
        f'<wp:docPr id="{doc_id}" name="{name}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="{name}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/>'
        f'<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>'
    )


def cell(text, w):
    return f'<w:tc><w:tcPr><w:tcW w:w="{w}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:t>{esc(text)}</w:t></w:r></w:p></w:tc>'


def table(rows, widths, style="TableGrid"):
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    body = "".join("<w:tr>" + "".join(cell(t, w) for t, w in zip(r, widths)) + "</w:tr>" for r in rows)
    return (
        f'<w:tbl><w:tblPr><w:tblStyle w:val="{style}"/><w:tblW w:w="0" w:type="auto"/>'
        f'<w:tblLook w:val="04A0"/></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{body}</w:tbl>'
    )


def text_box(doc_id, name, x, y, cx, cy, content, auto_fit, fill="FFF4D6", line="C08A00"):
    fit = "<a:spAutoFit/>" if auto_fit else "<a:noAutofit/>"
    vml_fit = ";mso-fit-shape-to-text:t" if auto_fit else ""
    choice = (
        f'<w:drawing><wp:anchor distT="45720" distB="45720" distL="114300" distR="114300" simplePos="0" '
        f'relativeHeight="{251659264 + doc_id}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1">'
        f'<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="margin"><wp:posOffset>{x * EMU}</wp:posOffset></wp:positionH>'
        f'<wp:positionV relativeFrom="paragraph"><wp:posOffset>{y * EMU}</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{cx * EMU}" cy="{cy * EMU}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>'
        f'<wp:wrapSquare wrapText="bothSides"/><wp:docPr id="{doc_id}" name="{name}"/><wp:cNvGraphicFramePr/>'
        '<a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp>'
        f'<wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx * EMU}" cy="{cy * EMU}"/></a:xfrm>'
        f'<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>'
        f'<a:ln w="9525"><a:solidFill><a:srgbClr val="{line}"/></a:solidFill></a:ln></wps:spPr>'
        f"<wps:txbx><w:txbxContent>{content}</w:txbxContent></wps:txbx>"
        f'<wps:bodyPr rot="0" vert="horz" wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" anchor="t" anchorCtr="0">{fit}</wps:bodyPr>'
        "</wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>"
    )
    fallback = (
        f'<w:pict><v:shape id="{name}" o:spid="_x0000_s{1024 + doc_id}" type="#_x0000_t202" '
        f'style="position:absolute;margin-left:{x * 0.75}pt;margin-top:{y * 0.75}pt;width:{cx * 0.75}pt;height:{cy * 0.75}pt;z-index:{doc_id}{vml_fit}" '
        f'fillcolor="#{fill}" strokecolor="#{line}"><v:textbox><w:txbxContent>{content}</w:txbxContent></v:textbox></v:shape></w:pict>'
    )
    return f'<w:r><mc:AlternateContent><mc:Choice Requires="wps">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent></w:r>'


def textboxes():
    rich = (
        para("Box with a picture and a table")
        + f"<w:p>{inline_picture('rIdBoxPic', 60 * EMU, 30 * EMU, 11, 'Box picture')}</w:p>"
        + table([["Item", "Qty"], ["Pens", "12"]], [1300, 1300])
        + para("After the table")
    )
    plain = para("Fits its text")
    body = (
        para("Text boxes", "Heading1")
        + "<w:p>"
        + text_box(1, "Rich box", 0, 0, 240, 40, rich, True)
        + f'<w:r><w:t xml:space="preserve">{LOREM * 4}</w:t></w:r></w:p>'
        + "<w:p>"
        + text_box(2, "Fit box", 0, 10, 200, 90, plain, True, fill="E2EFDA", line="548235")
        + f'<w:r><w:t xml:space="preserve">{LOREM * 3}</w:t></w:r></w:p>'
        + "".join(para(LOREM * 2) for _ in range(3))
    )
    return package(body, media=[("rIdBoxPic", "boxpic.png", png(60, 30, (200, 60, 60)))])


# ── formulas.docx ──────────────────────────────────────────────────────────


def fld(instr, result="0"):
    """A complex field with a stale result."""
    return (
        '<w:r><w:fldChar w:fldCharType="begin"/></w:r>'
        f'<w:r><w:instrText xml:space="preserve"> {esc(instr)} </w:instrText></w:r>'
        '<w:r><w:fldChar w:fldCharType="separate"/></w:r>'
        f'<w:r><w:t xml:space="preserve">{esc(result)}</w:t></w:r>'
        '<w:r><w:fldChar w:fldCharType="end"/></w:r>'
    )


def text(t):
    return f'<w:r><w:t xml:space="preserve">{esc(t)}</w:t></w:r>'


def bookmark(bid, name, inner):
    return f'<w:bookmarkStart w:id="{bid}" w:name="{name}"/>{inner}<w:bookmarkEnd w:id="{bid}"/>'


def fcell(content, w=1800):
    return f'<w:tc><w:tcPr><w:tcW w:w="{w}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:spacing w:after="0"/></w:pPr>{content}</w:p></w:tc>'


NUMBERING = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="multilevel"/>
<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1)"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="360" w:hanging="360"/></w:pPr></w:lvl>
<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2)"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl>
</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"""


def formulas():
    widths = [2400, 1800, 1800, 1800]
    rows = [
        [text("Item"), text("Qty"), text("Price"), text("Amount")],
        [text("Pens"), text("12"), text("1.50"), fld('= B2*C2 \\# "0.00"')],
        [text("Paper"), text("3"), text("4.25"), fld('= B3*C3 \\# "0.00"')],
        [text("Ink"), text("2"), text("10"), fld("= PRODUCT(B4:C4)")],
        [text("Total"), fld("=SUM(ABOVE)"), fld("=AVERAGE(LEFT)"), fld('= SUM(D2:D4) \\# "#,##0.00"')],
    ]
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    tbl = (
        '<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr>'
        f"<w:tblGrid>{grid}</w:tblGrid>"
        + "".join("<w:tr>" + "".join(fcell(c, w) for c, w in zip(r, widths)) + "</w:tr>" for r in rows)
        + "</w:tbl>"
    )
    numbered = lambda lvl, inner: (
        f'<w:p><w:pPr><w:numPr><w:ilvl w:val="{lvl}"/><w:numId w:val="1"/></w:numPr></w:pPr>{inner}</w:p>'
    )
    body = (
        # TOC limited to the bookmark "Part2" (\b) and one from TC entries (\f).
        para("Contents of part two")
        + "<w:p>" + fld('TOC \\o "1-2" \\b Part2 \\h', "stale") + "</w:p>"
        + para("Figures")
        + "<w:p>" + fld("TOC \\f F \\l 1-2", "stale") + "</w:p>"
        + '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>' + text("Part one") + "</w:p>"
        + "<w:p>" + text("Price ") + bookmark(1, "Price", text("250")) + text(", count ")
        + bookmark(2, "Count", text("4"))
        + text(". Total: ") + fld('= Price * Count \\# "#,##0"') + text(". Half: ") + fld("=Price/2")
        + text(". Rounded: ") + fld("=ROUND(10/3, 2)") + text(". Check: ") + fld("=IF(Count > 3, 1, 0)")
        + text(". Max: ") + fld("=MAX(3, 8, Count)") + text(". ") + fld("= 2 + 3 * 4") + "</w:p>"
        + "<w:p>" + text("If: ") + fld('IF 4 > 3 "many" "few"')
        + text(" / ") + fld('IF "yes" = "no" "same" "different"')
        + text(" / compare: ") + fld("COMPARE 5 >= 3") + text(" / ") + fld('COMPARE "abc" = "a*"') + "</w:p>"
        + "<w:p>" + fld("SET Rate 0.11", "") + text("Rate is ") + fld("REF Rate") + text(" / tax ")
        + fld("= Rate * 100") + "</w:p>"
        + "<w:p>" + text("Style ref: ") + fld('STYLEREF "Heading 1"') + text(" / ") + fld("STYLEREF 1") + "</w:p>"
        + tbl
        + '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>' + text("Part two") + "</w:p>"
        + bookmark(
            3,
            "Part2",
            '<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr>' + text("Inside the bookmark") + "</w:p>"
            + "<w:p>" + fld('TC "Figure entry A" \\f F \\l 1', "") + text("Seq ") + fld("SEQ Step")
            + text(", ") + fld("SEQ Step") + text(", restart ") + fld("SEQ Step \\r 10")
            + text(", same ") + fld("SEQ Step \\c") + text(", hidden") + fld("SEQ Step \\h")
            + text(", next ") + fld("SEQ Step") + "</w:p>",
        )
        + '<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr>' + text("Outside the bookmark") + "</w:p>"
        + "<w:p>" + fld('TC "Figure entry B" \\f F \\l 2', "") + text("Style ref here: ")
        + fld('STYLEREF "Heading 2"') + "</w:p>"
        + numbered(0, text("First ") + fld("LISTNUM") + text(" and ") + fld("LISTNUM"))
        + numbered(1, text("Second ") + fld("LISTNUM \\l 1 \\s 5"))
        + "<w:p>" + text("Index: ") + fld("INDEX \\c 1", "stale") + "</w:p>"
        + "<w:p>" + text("Entry ") + fld('XE "Apples"', "") + text("and ") + fld('XE "Bananas"', "")
        + text("and ") + fld('XE "Apples"', "") + "</w:p>"
    )
    return package(body, numbering=NUMBERING)


# ── styleref_index.docx ────────────────────────────────────────────────────

PAGE_BREAK = '<w:r><w:br w:type="page"/></w:r>'


def styleref_index():
    h1 = lambda t: f'<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{text(t)}</w:p>'
    xe = lambda t, extra="": fld(f'XE "{t}"{extra}', "")
    body = (
        h1("Alpha chapter")
        + "<w:p>" + text("Apples grow here.") + xe("Apples") + xe("Fruit:Apples") + "</w:p>"
        + "<w:p>" + text("Zebras too.") + xe("Zebras") + PAGE_BREAK + "</w:p>"
        + "<w:p>" + text("Still the first chapter, no heading on this page.") + xe("Bananas")
        + xe("Fruit", ' \\t "See Apples"') + PAGE_BREAK + "</w:p>"
        + h1("Beta chapter")
        + "<w:p>" + text("Apples again.") + xe("Apples") + xe("Avocados") + PAGE_BREAK + "</w:p>"
        + "<w:p>" + fld('INDEX \\h "A" \\e ", "', "stale") + "</w:p>"
    )
    header = "<w:p>" + text("Chapter: ") + fld('STYLEREF "Heading 1"') + "</w:p>"
    return package(body, header=header)


FILES = {"textboxes.docx": textboxes, "formulas.docx": formulas, "styleref_index.docx": styleref_index}


def main():
    args = [a for a in sys.argv[1:] if a != "--check"]
    out = Path(args[0] if args else "tests/corpus/docx")
    check = "--check" in sys.argv
    bad = 0
    for name, make in FILES.items():
        data = make()
        path = out / name
        if check:
            if not path.exists() or path.read_bytes() != data:
                print(f"{path} differs from what gen_docx_more.py writes")
                bad += 1
        else:
            path.write_bytes(data)
    if bad:
        sys.exit(1)
    print("docx (more) ok")


if __name__ == "__main__":
    main()
