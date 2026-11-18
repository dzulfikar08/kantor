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
* columns_long.docx: about 20 pages of exact 14 pt lines in two equal
  columns (with a line between), three columns and two unequal ones,
  continuous section breaks between them, footnotes, exact-height tables and
  pictures: checked against LibreOffice 25.8 (crates/docs_pdf/tests).
* styleref_index.docx: a header with `STYLEREF "Heading 1"` over four
  pages (two chapters, a page without a heading, the index page), and an
  `INDEX \\h "A"` of `XE` entries with sub-entries and a cross-reference
  (`\\t "See Apples"`). Results saved wrong ("0").

* pictures.docx: inline pictures cut to their shapes (`a:prstGeom` oval,
  rounded rectangle, hexagon, five-point star) with outlines (`a:ln`, one
  dashed), a rectangle with a border, a turned oval, an oval picture in the
  header, a floating oval picture, and three SmartArt picture layouts whose
  shapes are filled with a picture from the drawing part's own
  relationships (Horizontal Picture List, Bending Picture Accent List,
  Circular Picture Callout; their parts taken from
  tests/corpus/pptx/smartart.pptx, drawn at 0.6 times its size). The
  second SmartArt (top-and-bottom wrapping) doesn't fit under the first
  and moves to page 2 with its paragraph.

* smartart_inline.docx: SmartArt in line with the text (`wp:inline`): a
  Horizontal Picture List in a paragraph of its own and a small Bending
  Picture Accent List inside a line of text; then a 2.5 in picture with
  top-and-bottom wrapping that doesn't fit at the bottom of page 1 and
  moves to page 2 with its paragraph (as Word and LibreOffice lay it out).

Regenerate with `python3 scripts/corpus/gen_docx_more.py tests/corpus/docx`;
`--check` compares with the files in that folder instead.
"""
import io
import re
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


def package(body, media=(), numbering=None, header=None, sect=None, footnotes=None, settings=None, extra=None):
    """A .docx's bytes: `body` inside w:body, `media` = [(rId, name, bytes)],
    `footnotes` the w:footnote elements of a footnotes part (separators
    added), `extra` = (more document relationships, more content type
    overrides, more parts {name: bytes or text})."""
    extra_rels, extra_overrides, extra_files = extra or ((), "", {})
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
    rels.extend(extra_rels)
    overrides += extra_overrides
    sect = sect or SECT
    if header:
        rels.append(
            '<Relationship Id="rIdHeader1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>'
        )
        sect = SECT.replace("<w:sectPr>", '<w:sectPr><w:headerReference w:type="default" r:id="rIdHeader1"/>')
    if settings:
        rels.append(
            '<Relationship Id="rIdSettings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>'
        )
        overrides += '<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>'
    if footnotes:
        rels.append(
            '<Relationship Id="rIdFootnotes" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/>'
        )
        overrides += '<Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"/>'
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
    if settings:
        files["word/settings.xml"] = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings {NS}>{settings}</w:settings>'
    if footnotes:
        files["word/footnotes.xml"] = (
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes {NS}>'
            '<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote>'
            '<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>'
            f"{footnotes}</w:footnotes>"
        )
    if header:
        files["word/header1.xml"] = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr {NS}>{header}</w:hdr>'
        files["[Content_Types].xml"] = files["[Content_Types].xml"].replace(
            "</Types>",
            '<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/></Types>',
        )
    for _, name, data in media:
        files[f"word/media/{name}"] = data
    files.update(extra_files)
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


# ── columns.docx ───────────────────────────────────────────────────────────
# Lines of exactly 14 pt and no paragraph spacing, short enough never to
# wrap: page counts don't depend on fonts (49 lines fit a column of A4
# with 2.54 cm margins), so Kantor's can be checked against LibreOffice's.


def col_sect(cols, kind=None):
    """A section's properties: A4, 2.54 cm margins, `cols` (a w:cols)."""
    t = f'<w:type w:val="{kind}"/>' if kind else ""
    return (
        f'<w:sectPr>{t}<w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" '
        f'w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/>{cols}</w:sectPr>'
    )


def line(t, before="", sect=""):
    ppr = f'<w:pPr><w:spacing w:before="0" w:after="0" w:line="280" w:lineRule="exact"/>{sect}</w:pPr>'
    return f'<w:p>{ppr}{before}<w:r><w:t xml:space="preserve">{esc(t)}</w:t></w:r></w:p>'


def columns():
    one = '<w:cols w:space="720"/>'
    two = '<w:cols w:num="2" w:sep="1" w:space="720"/>'
    left = (
        '<w:cols w:num="2" w:space="720" w:equalWidth="0">'
        '<w:col w:w="2600" w:space="720"/><w:col w:w="5706"/></w:cols>'
    )
    col_break = '<w:r><w:br w:type="column"/></w:r>'
    # 1: a title and two lines, one column.
    body = line("Columns") + line("One column first.")
    body += line("Then two, continuously.", sect=col_sect(one))
    # 2: 130 items in two columns with a line between; a column break
    # before item 21 (at the start of its paragraph: LibreOffice drops the
    # empty line Word and Kantor show for a break ending a paragraph).
    # Balanced on its last page before the continuous break.
    for i in range(1, 131):
        extra = col_break if i == 21 else ""
        sect = col_sect(two, "continuous") if i == 130 else ""
        body += line(f"Item {i}", extra, sect)
    # 3: five lines in one column, under the balanced columns.
    for i in range(1, 6):
        sect = col_sect(one, "continuous") if i == 5 else ""
        body += line(f"After {i}", sect=sect)
    # 4 (the body's): a new page, a narrow left column and a wide one.
    for i in range(1, 61):
        body += line(f"Left {i}")
    return package(body, sect=col_sect(left))


# ── columns_long.docx ──────────────────────────────────────────────────────
# 20-odd pages of the same 14 pt exact lines in 2 and 3 columns (equal and
# unequal), continuous section breaks between them, footnotes, tables with
# exact-height rows and pictures in exact-height lines: page breaks and
# line places don't depend on fonts, so Kantor's can be compared with
# LibreOffice's line for line (tools: ~/.cache LibreOffice, pdftotext).


def note_ref(n):
    return (
        '<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr>'
        f'<w:footnoteReference w:id="{n}"/></w:r>'
    )


def footnote(n, t):
    return (
        f'<w:footnote w:id="{n}"><w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" '
        'w:lineRule="exact"/></w:pPr><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteRef/></w:r>'
        f'<w:r><w:t xml:space="preserve"> {esc(t)}</w:t></w:r></w:p></w:footnote>'
    )


def exact_table(label, rows, widths):
    """A Table Grid table, rows exactly 14 pt, no cell padding top/bottom."""
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    body = ""
    for r in range(rows):
        cells = ""
        for c, w in enumerate(widths):
            cells += (
                f'<w:tc><w:tcPr><w:tcW w:w="{w}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:spacing w:before="0" '
                'w:after="0" w:line="280" w:lineRule="exact"/></w:pPr>'
                f'<w:r><w:t>{esc(label)} {r + 1}.{c + 1}</w:t></w:r></w:p></w:tc>'
            )
        body += f'<w:tr><w:trPr><w:cantSplit/><w:trHeight w:val="280" w:hRule="exact"/></w:trPr>{cells}</w:tr>'
    return (
        '<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/>'
        '<w:tblLook w:val="04A0"/></w:tblPr>'
        f"<w:tblGrid>{grid}</w:tblGrid>{body}</w:tbl>"
    )


def picture_line(rid, doc_id):
    """An inline 1.5 x 0.75 in picture in a line of exactly 1.5 in (room
    above the baseline for it whatever the font)."""
    cx, cy = 1371600, 685800
    return (
        '<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="2160" w:lineRule="exact"/></w:pPr>'
        + inline_picture(rid, cx, cy, doc_id, f"Picture {doc_id}")
        + "</w:p>"
    )


def columns_long():
    one = '<w:cols w:space="720"/>'
    two = '<w:cols w:num="2" w:sep="1" w:space="720"/>'
    three = '<w:cols w:num="3" w:space="567"/>'
    left = (
        '<w:cols w:num="2" w:space="720" w:equalWidth="0">'
        '<w:col w:w="2600" w:space="720"/><w:col w:w="5706"/></w:cols>'
    )
    notes = []
    pics = 0

    def items(prefix, n, note_every, table_every, pic_every, widths):
        nonlocal pics
        out = ""
        for i in range(1, n + 1):
            ref = ""
            if note_every and i % note_every == 0:
                notes.append(footnote(len(notes) + 1, f"Note on {prefix} {i}"))
                ref = note_ref(len(notes))
            out += line(f"{prefix} {i}") if not ref else line(f"{prefix} {i}")[:-6] + ref + "</w:p>"
            if table_every and i % table_every == 0:
                out += exact_table(f"{prefix}{i}", 4, widths)
                out += line(f"{prefix} {i} after table")
            if pic_every and i % pic_every == 0:
                pics += 1
                out += picture_line("rIdLongPic", 100 + pics)
        return out

    body = line("Long columns") + line("One column, then two.", sect=col_sect(one))
    # Two equal columns with a line between: notes, tables, pictures.
    body += items("A", 700, 37, 90, 120, [1800, 1800])
    body += line("A end", sect=col_sect(two, "continuous"))
    # One column between, five lines.
    for i in range(1, 6):
        body += line(f"Between {i}", sect=col_sect(one, "continuous") if i == 5 else "")
    # Three columns.
    body += items("B", 900, 53, 110, 150, [1100, 1100])
    body += line("B end", sect=col_sect(three, "continuous"))
    # A narrow left column and a wide one, from a new page.
    body += items("C", 300, 41, 0, 100, [])
    body += line("C end", sect=col_sect(left, "nextPage"))
    for i in range(1, 11):
        body += line(f"Last {i}")
    media = [("rIdLongPic", "long.png", png(36, 18, (90, 140, 200)))]
    # Word 2013's table placement (the border at the margin), as Kantor.
    settings = '<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat>'
    return package(body, media=media, sect=col_sect(one), footnotes="".join(notes), settings=settings)


# ── pictures.docx ──────────────────────────────────────────────────────────

REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
THEME = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements>'
    '<a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>'
    '<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2>'
    '<a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1>'
    '<a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>'
    '<a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>'
    '<a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink>'
    '<a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>'
    '<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>'
    '<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>'
    '<a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>'
    '<a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>'
    '<a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>'
    '<a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>'
    '<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle>'
    '<a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>'
    "</a:fmtScheme></a:themeElements></a:theme>"
)


def checker(w, h):
    """A w x h PNG: four coloured quarters with a light grid, so a cut or
    turn shows."""
    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    quarters = [(230, 80, 60), (60, 160, 90), (70, 110, 210), (240, 190, 50)]
    rows = b""
    for y in range(h):
        row = b"\x00"
        for x in range(w):
            if x % 16 == 0 or y % 16 == 0:
                row += bytes((250, 250, 250))
            else:
                row += bytes(quarters[(y * 2 // h) * 2 + x * 2 // w])
        rows += row
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


def shaped_picture(rid, cx, cy, doc_id, name, prst, ln="", rot=0):
    """An inline picture cut to preset `prst`, with outline `ln`."""
    r = f' rot="{rot}"' if rot else ""
    return (
        f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="114300" distR="114300"><wp:extent cx="{cx}" cy="{cy}"/>'
        f'<wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{doc_id}" name="{name}"/>'
        f'<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>'
        f'<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{doc_id}" name="{name}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/>'
        f'<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm{r}><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom>{ln}</pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>'
    )


def solid_ln(w_pt, color, dash=None):
    d = f'<a:prstDash val="{dash}"/>' if dash else ""
    return f'<a:ln w="{int(w_pt * 12700)}"><a:solidFill><a:srgbClr val="{color}"/></a:solidFill>{d}</a:ln>'


def floating_oval(rid, doc_id):
    cx = cy = 96 * EMU
    return (
        f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="{251659264 + doc_id}" '
        'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
        '<wp:positionH relativeFrom="margin"><wp:align>right</wp:align></wp:positionH>'
        '<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapSquare wrapText="bothSides"/>'
        f'<wp:docPr id="{doc_id}" name="Floating oval"/><wp:cNvGraphicFramePr/>'
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{doc_id}" name="float.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/>'
        f'<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>{solid_ln(2.25, "7030A0")}</pic:spPr></pic:pic>'
        "</a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"
    )


SMARTART_PPTX = Path(__file__).resolve().parents[2] / "tests/corpus/pptx/smartart.pptx"
# (slide in smartart.pptx, title): the picture layouts.
SMARTART_PICS = [(10, "Horizontal Picture List"), (13, "Bending Picture Accent List"), (18, "Circular Picture Callout")]
SA_SCALE = 0.6


def scale_drawing(xml, scale=SA_SCALE):
    """smartart.pptx's 10 x 5 in drawing at `scale` times its size."""
    def num(m):
        return f'{m.group(1)}="{round(int(m.group(2)) * scale)}"'

    def xfrm(m):
        return re.sub(r'\b(x|y|cx|cy)="(-?\d+)"', num, m.group(0))

    return re.sub(r"<a:(off|ext) [^>]*/>", xfrm, xml)


def smartart_frame(n, ids, w, h, inline):
    """The run holding SmartArt `n`: a top-and-bottom anchor centred in the
    column, or in line with the text (`wp:inline`, as Word inserts it)."""
    graphic = (
        f'<wp:docPr id="{40 + n}" name="Diagram {n}"/><wp:cNvGraphicFramePr/>'
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">'
        f'<dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="{ids["Dm"]}" '
        f'r:lo="{ids["Lo"]}" r:qs="{ids["Qs"]}" r:cs="{ids["Cs"]}"/></a:graphicData></a:graphic>'
    )
    if inline:
        return (
            '<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">'
            f'<wp:extent cx="{w}" cy="{h}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>'
            f"{graphic}</wp:inline></w:drawing></w:r>"
        )
    return (
        f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="{251660000 + n}" '
        'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
        '<wp:positionH relativeFrom="column"><wp:align>center</wp:align></wp:positionH>'
        '<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{w}" cy="{h}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapTopAndBottom/>'
        f"{graphic}</wp:anchor></w:drawing></w:r>"
    )


def smartart_parts(pics=SMARTART_PICS, scales=None, inline=()):
    """For each picture layout of `pics` (drawn at `scales[k]` times
    smartart.pptx's size, default SA_SCALE; in line when `k in inline`):
    (title, run XML), rels, overrides, files."""
    src = zipfile.ZipFile(SMARTART_PPTX)
    rels, overrides, files, runs = [], "", {}, []
    ct = "application/vnd.openxmlformats-officedocument.drawingml."
    for k, (slide, title) in enumerate(pics):
        n = k + 1
        scale = scales[k] if scales else SA_SCALE
        w, h = round(10 * 914400 * scale), round(5 * 914400 * scale)
        ids = {kind: f"rIdSa{n}{kind}" for kind in ("Dm", "Lo", "Qs", "Cs", "Dr")}
        data = src.read(f"ppt/diagrams/data{slide}.xml").decode().replace('relId="rIdDr"', f'relId="{ids["Dr"]}"')
        files[f"word/diagrams/data{n}.xml"] = data
        files[f"word/diagrams/layout{n}.xml"] = src.read(f"ppt/diagrams/layout{slide}.xml")
        files[f"word/diagrams/quickStyle{n}.xml"] = src.read(f"ppt/diagrams/quickStyle{slide}.xml")
        files[f"word/diagrams/colors{n}.xml"] = src.read(f"ppt/diagrams/colors{slide}.xml")
        files[f"word/diagrams/drawing{n}.xml"] = scale_drawing(src.read(f"ppt/diagrams/drawing{slide}.xml").decode(), scale)
        # The drawing's picture: its own relationship (rIdImg1).
        files[f"word/diagrams/_rels/drawing{n}.xml.rels"] = src.read(f"ppt/diagrams/_rels/drawing{slide}.xml.rels")
        for kind, ty, part in [
            ("Dm", "diagramData", "data"),
            ("Lo", "diagramLayout", "layout"),
            ("Qs", "diagramQuickStyle", "quickStyle"),
            ("Cs", "diagramColors", "colors"),
        ]:
            rels.append(f'<Relationship Id="{ids[kind]}" Type="{REL}{ty}" Target="diagrams/{part}{n}.xml"/>')
        rels.append(
            f'<Relationship Id="{ids["Dr"]}" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" '
            f'Target="diagrams/drawing{n}.xml"/>'
        )
        for part, kind in [("data", "diagramData"), ("layout", "diagramLayout"), ("quickStyle", "diagramStyle"), ("colors", "diagramColors")]:
            overrides += f'<Override PartName="/word/diagrams/{part}{n}.xml" ContentType="{ct}{kind}+xml"/>'
        overrides += f'<Override PartName="/word/diagrams/drawing{n}.xml" ContentType="application/vnd.ms-office.drawingml.diagramDrawing+xml"/>'
        runs.append((title, smartart_frame(n, ids, w, h, k in inline)))
    files["word/media/smartart_pic1.png"] = src.read("ppt/media/smartart_pic1.png")
    return runs, rels, overrides, files


def pictures():
    px = 110 * EMU
    shapes = [
        ("rIdPic", "Oval", "ellipse", solid_ln(3, "C00000"), 0, px, round(px * 0.75)),
        ("rIdPic", "Rounded", "roundRect", solid_ln(2.25, "2F5597"), 0, px, round(px * 0.75)),
        ("rIdPic", "Hexagon", "hexagon", solid_ln(1.5, "385723", "dash"), 0, px, round(px * 0.75)),
        ("rIdPic", "Star", "star5", "", 0, px, px),
    ]
    line1 = "".join(shaped_picture(rid, cx, cy, 10 + k, n, prst, ln, rot) for k, (rid, n, prst, ln, rot, cx, cy) in enumerate(shapes))
    line2 = (
        shaped_picture("rIdPic", px, round(px * 0.75), 20, "Bordered", "rect", solid_ln(4.5, "000000"))
        + shaped_picture("rIdPic", px, round(px * 0.6), 21, "Turned oval", "ellipse", solid_ln(2.25, "C55A11"), rot=30 * 60000)
    )
    runs, rels, overrides, files = smartart_parts()
    body = (
        para("Pictures cut to shapes", "Heading1")
        + para("Inline pictures cut to an oval, a rounded rectangle, a hexagon (dashed outline) and a star:")
        + f"<w:p>{line1}</w:p>"
        + para("A rectangle with a thick border, and an oval turned 30 degrees:")
        + f"<w:p>{line2}</w:p>"
        + f'<w:p>{floating_oval("rIdPic", 30)}<w:r><w:t xml:space="preserve">{LOREM * 3}</w:t></w:r></w:p>'
    )
    # The second SmartArt doesn't fit under the first: it moves to the
    # next page with its paragraph, as in Word.
    for title, run in runs:
        body += para(title, "Heading1") + f"<w:p>{run}</w:p>" + para(f"After the {title}.")
    header = (
        '<w:p><w:pPr><w:spacing w:after="0"/></w:pPr>'
        + shaped_picture("rIdHdrPic", 40 * EMU, 40 * EMU, 50, "Header oval", "ellipse", solid_ln(1.5, "2F5597"))
        + '<w:r><w:t xml:space="preserve"> Pictures letterhead</w:t></w:r></w:p>'
    )
    files["word/_rels/header1.xml.rels"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f'<Relationship Id="rIdHdrPic" Type="{REL}image" Target="media/pic.png"/></Relationships>'
    )
    files["word/theme/theme1.xml"] = THEME
    rels.append(f'<Relationship Id="rIdTheme" Type="{REL}theme" Target="theme/theme1.xml"/>')
    overrides += '<Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
    return package(
        body,
        media=[("rIdPic", "pic.png", checker(128, 96))],
        header=header,
        extra=(rels, overrides, files),
    )


# ── smartart_inline.docx ───────────────────────────────────────────────────


def top_bottom_picture(rid, doc_id, cx, cy):
    """A floating picture with top-and-bottom wrapping, centred in the
    column at the top of its paragraph."""
    return (
        f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="{251661000 + doc_id}" '
        'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
        '<wp:positionH relativeFrom="column"><wp:align>center</wp:align></wp:positionH>'
        '<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapTopAndBottom/>'
        f'<wp:docPr id="{doc_id}" name="Top and bottom picture"/><wp:cNvGraphicFramePr/>'
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{doc_id}" name="block.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/>'
        f'<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>'
        "</a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"
    )


def smartart_inline():
    pics = [(10, "Horizontal Picture List"), (13, "Bending Picture Accent List")]
    runs, rels, overrides, files = smartart_parts(pics, scales=[0.6, 0.25], inline=(0, 1))
    (_, big), (_, small) = runs
    body = (
        para("SmartArt in line with text", "Heading1")
        + para("A Horizontal Picture List in line with the text, as Word inserts SmartArt:")
        + f'<w:p><w:pPr><w:jc w:val="center"/></w:pPr>{big}</w:p>'
        + para("After the inline SmartArt.")
        + '<w:p><w:r><w:t xml:space="preserve">A small Bending Picture Accent List </w:t></w:r>'
        + small
        + '<w:r><w:t xml:space="preserve"> sits in this line of text like a picture.</w:t></w:r></w:p>'
        + "".join(para(LOREM * 2) for _ in range(3))
        + para("The next paragraph holds a picture with top-and-bottom wrapping, 2.5 inches tall:")
        + f'<w:p>{top_bottom_picture("rIdBlock", 60, 300 * EMU, 240 * EMU)}'
        + '<w:r><w:t xml:space="preserve">The picture above wraps top and bottom and doesn\'t fit at the bottom of '
        + "the first page, so it moves to the next page with this paragraph, as in Word.</w:t></w:r></w:p>"
        + para("After the moved picture.")
    )
    files["word/theme/theme1.xml"] = THEME
    rels.append(f'<Relationship Id="rIdTheme" Type="{REL}theme" Target="theme/theme1.xml"/>')
    overrides += '<Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
    return package(body, media=[("rIdBlock", "block.png", checker(160, 128))], extra=(rels, overrides, files))


FILES = {
    "textboxes.docx": textboxes,
    "formulas.docx": formulas,
    "styleref_index.docx": styleref_index,
    "columns.docx": columns,
    "columns_long.docx": columns_long,
    "pictures.docx": pictures,
    "smartart_inline.docx": smartart_inline,
}


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
