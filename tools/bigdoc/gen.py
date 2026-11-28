#!/usr/bin/env python3
"""Reproducible 1,000+ page Word documents for timing Kantor Docs.

    python3 -I tools/bigdoc/gen.py [OUTDIR] [--scale F] [--only NAME]

writes (default OUTDIR ~/.cache/bigdoc):

  contract.docx  a regulation/contract compilation: cover with a different
                 first page, a table of contents field, three numbered
                 heading levels (multilevel list), clause lists (a)/(i)
                 restarted per article, cross-references (REF \\r and
                 PAGEREF to heading bookmarks), "Page X of Y" headers,
                 several sections incl. landscape schedules with wide tables.
  thesis.docx    a thesis/manual: roman-numbered front matter, chapters with
                 STYLEREF running headers, a picture every few pages
                 (JPEG photos made by ImageMagick, PNG charts), captions
                 with SEQ fields, footnotes, long tables with repeated header
                 rows, floating pictures with square wrap.
  reviewed.docx  contract.docx after review: hundreds of tracked insertions,
                 deletions and formatting changes by three authors, and
                 comments with replies.

Each has the words MIDPOINTMARK (a paragraph in the middle) and
ENDPOINTMARK (near the end), to put the caret there with Find.

Only the standard library is needed; pictures are JPEGs from ImageMagick
(`magick`, plasma fractals with fixed seeds) when it is installed, else
PNGs written here. The same arguments always give the same files.
"""

import argparse
import os
import random
import shutil
import struct
import subprocess
import sys
import zipfile
import zlib
from xml.sax.saxutils import escape

W_NS = (
    'xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" '
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
    'xmlns:o="urn:schemas-microsoft-com:office:office" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
    'xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" '
    'xmlns:v="urn:schemas-microsoft-com:vml" '
    'xmlns:wp14="http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" '
    'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
    'xmlns:w10="urn:schemas-microsoft-com:office:word" '
    'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
    'xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" '
    'xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" '
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" '
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
    'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" '
    'mc:Ignorable="w14 w15 wp14"'
)
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
DATE = "2026-10-01T09:00:00Z"

WORDS = (
    "the party shall provide notice within thirty days of any material change "
    "agreement pursuant to section hereof subject to applicable law regulation "
    "obligations rights remedies contractor employer owner authority approval "
    "consent writing reasonable efforts compliance standards specification works "
    "services payment invoice period term termination default breach liability "
    "indemnity insurance warranty defect completion acceptance certificate "
    "variation instruction schedule programme delay extension cost claim dispute "
    "resolution arbitration jurisdiction confidential information property "
    "intellectual license assignment subcontract personnel safety environment "
    "quality assurance inspection testing records reporting audit review measure "
    "valuation retention security bond guarantee force majeure event circumstance "
    "notwithstanding foregoing provided that unless otherwise agreed in respect of "
    "including without limitation any all each such other relevant respective "
    "prior written further time manner form accordance with under by for to from"
).split()
THESIS_WORDS = (
    "the results show that model data analysis method approach study research "
    "significant effect between variables sample measurement observed expected "
    "values distribution mean variance regression correlation experiment design "
    "performance system process material structure temperature pressure flow "
    "energy efficiency optimization algorithm simulation validation calibration "
    "framework theory literature previous work however therefore moreover while "
    "this chapter presents describes discusses evaluates compares proposed "
    "baseline condition parameter estimate error uncertainty figure table section "
    "in of for with on by as from at into across within under over the a an and"
).split()


def sentence(rng, words, lo=10, hi=26):
    n = rng.randint(lo, hi)
    s = " ".join(rng.choice(words) for _ in range(n))
    return s[0].upper() + s[1:] + "."


def para_text(rng, words, lo=2, hi=5):
    return " ".join(sentence(rng, words) for _ in range(rng.randint(lo, hi)))


def t(text):
    return f'<w:t xml:space="preserve">{escape(text)}</w:t>'


def run(text, rpr=""):
    return f"<w:r>{('<w:rPr>' + rpr + '</w:rPr>') if rpr else ''}{t(text)}</w:r>"


def field(code, result, rpr=""):
    """A complex field with a cached result."""
    rp = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    return (
        f'<w:r>{rp}<w:fldChar w:fldCharType="begin"/></w:r>'
        f'<w:r>{rp}<w:instrText xml:space="preserve"> {escape(code)} </w:instrText></w:r>'
        f'<w:r>{rp}<w:fldChar w:fldCharType="separate"/></w:r>'
        f"<w:r>{rp}{t(result)}</w:r>"
        f'<w:r>{rp}<w:fldChar w:fldCharType="end"/></w:r>'
    )


def para(body, style=None, ppr=""):
    st = f'<w:pStyle w:val="{style}"/>' if style else ""
    pp = f"<w:pPr>{st}{ppr}</w:pPr>" if (st or ppr) else ""
    return f"<w:p>{pp}{body}</w:p>"


class Pkg:
    """Parts of a .docx being written."""

    def __init__(self):
        self.rels = []  # (id, type, target, extra)
        self.media = {}  # name -> bytes
        self.parts = {}  # path -> xml
        self.overrides = {}  # path -> content type
        self.next_rel = 10

    def rel(self, typ, target, extra=""):
        rid = f"rId{self.next_rel}"
        self.next_rel += 1
        self.rels.append((rid, typ, target, extra))
        return rid

    def add_part(self, path, xml, ctype, reltype=None):
        self.parts[path] = xml
        self.overrides["/" + path] = ctype
        if reltype:
            return self.rel(reltype, path[len("word/"):])
        return None

    def write(self, path, document_xml, title):
        ct = [
            XML,
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">',
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>',
            '<Default Extension="xml" ContentType="application/xml"/>',
            '<Default Extension="png" ContentType="image/png"/>',
            '<Default Extension="jpeg" ContentType="image/jpeg"/>',
            '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>',
            '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>',
            '<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>',
        ]
        for p, c in sorted(self.overrides.items()):
            ct.append(f'<Override PartName="{p}" ContentType="{c}"/>')
        ct.append("</Types>")
        rels = [XML, f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">']
        for rid, typ, target, extra in self.rels:
            uri = typ if typ.startswith("http") else f"{REL}/{typ}"
            rels.append(f'<Relationship Id="{rid}" Type="{uri}" Target="{target}"{extra}/>')
        rels.append("</Relationships>")
        root_rels = (
            XML
            + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f'<Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/>'
            '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>'
            f'<Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/>'
            "</Relationships>"
        )
        core = (
            XML
            + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
            'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
            'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
            f"<dc:title>{escape(title)}</dc:title><dc:creator>bigdoc generator</dc:creator>"
            f'<dcterms:created xsi:type="dcterms:W3CDTF">{DATE}</dcterms:created>'
            f'<dcterms:modified xsi:type="dcterms:W3CDTF">{DATE}</dcterms:modified>'
            "</cp:coreProperties>"
        )
        app = (
            XML
            + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">'
            "<Application>Microsoft Office Word</Application></Properties>"
        )
        tmp = path + ".part"
        with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:

            def put(name, data, compress=True):
                info = zipfile.ZipInfo(name, date_time=(2026, 10, 1, 9, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED if compress else zipfile.ZIP_STORED
                z.writestr(info, data)

            put("[Content_Types].xml", "".join(ct))
            put("_rels/.rels", root_rels)
            put("docProps/core.xml", core)
            put("docProps/app.xml", app)
            put("word/document.xml", document_xml)
            put("word/_rels/document.xml.rels", "".join(rels))
            for p, x in sorted(self.parts.items()):
                put(p, x)
            for name, data in sorted(self.media.items()):
                put("word/media/" + name, data, compress=False)
        os.replace(tmp, path)


def styles_xml(heading_font="Cambria", body_font="Calibri"):
    def heading(level, size, num_lvl):
        return (
            f'<w:style w:type="paragraph" w:styleId="Heading{level}"><w:name w:val="heading {level}"/>'
            f'<w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/>'
            f'<w:pPr><w:keepNext/><w:keepLines/><w:numPr><w:ilvl w:val="{num_lvl}"/><w:numId w:val="1"/></w:numPr>'
            f'<w:spacing w:before="{360 - level * 60}" w:after="120"/><w:outlineLvl w:val="{level - 1}"/></w:pPr>'
            f'<w:rPr><w:rFonts w:ascii="{heading_font}" w:hAnsi="{heading_font}"/><w:b/><w:sz w:val="{size}"/></w:rPr></w:style>'
        )

    toc = "".join(
        f'<w:style w:type="paragraph" w:styleId="TOC{i}"><w:name w:val="toc {i}"/><w:basedOn w:val="Normal"/>'
        f'<w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9350"/></w:tabs>'
        f'<w:spacing w:after="60"/><w:ind w:left="{(i - 1) * 220}"/></w:pPr></w:style>'
        for i in (1, 2, 3)
    )
    return (
        XML
        + f'<w:styles {W_NS}>'
        f'<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{body_font}" w:hAnsi="{body_font}" w:cs="{body_font}"/>'
        '<w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-GB"/></w:rPr></w:rPrDefault>'
        '<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>'
        '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/>'
        '<w:pPr><w:jc w:val="both"/></w:pPr></w:style>'
        '<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/></w:style>'
        + heading(1, 32, 0)
        + heading(2, 26, 1)
        + heading(3, 23, 2)
        + '<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/>'
        f'<w:pPr><w:jc w:val="center"/><w:spacing w:before="2400" w:after="480"/></w:pPr><w:rPr><w:rFonts w:ascii="{heading_font}" w:hAnsi="{heading_font}"/><w:sz w:val="56"/></w:rPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="Subtitle"><w:name w:val="Subtitle"/><w:basedOn w:val="Normal"/>'
        '<w:pPr><w:jc w:val="center"/></w:pPr><w:rPr><w:color w:val="595959"/><w:sz w:val="28"/></w:rPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="TOCHeading"><w:name w:val="TOC Heading"/><w:basedOn w:val="Normal"/>'
        '<w:pPr><w:spacing w:before="240" w:after="240"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>'
        + toc
        + '<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/>'
        '<w:pPr><w:spacing w:after="80"/><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="Caption"><w:name w:val="caption"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/>'
        '<w:pPr><w:jc w:val="center"/><w:spacing w:after="200"/></w:pPr><w:rPr><w:i/><w:color w:val="44546A"/><w:sz w:val="18"/></w:rPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="Header"><w:name w:val="header"/><w:basedOn w:val="Normal"/>'
        '<w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:after="0"/></w:pPr><w:rPr><w:sz w:val="18"/></w:rPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/><w:basedOn w:val="Normal"/>'
        '<w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs><w:spacing w:after="0"/></w:pPr><w:rPr><w:sz w:val="18"/></w:rPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/>'
        '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/></w:rPr></w:style>'
        '<w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>'
        '<w:style w:type="paragraph" w:styleId="CommentText"><w:name w:val="annotation text"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="20"/></w:rPr></w:style>'
        '<w:style w:type="character" w:styleId="CommentReference"><w:name w:val="annotation reference"/><w:rPr><w:sz w:val="16"/></w:rPr></w:style>'
        '<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>'
        '<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/>'
        '<w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/>'
        '<w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>'
        '<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/>'
        '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:jc w:val="left"/></w:pPr>'
        '<w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/>'
        '<w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/>'
        '<w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>'
        "</w:styles>"
    )


def numbering_xml(n_clause_lists, n_bullet_lists=0):
    """abstractNum 0: headings (Article 1 / 1.1 / 1.1.1); 1: clauses
    (a)/(i)/(A); 2: bullets. One w:num per clause list (restarted)."""
    lvls_h = (
        '<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="Heading1"/>'
        '<w:lvlText w:val="Article %1"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="1440" w:hanging="1440"/></w:pPr></w:lvl>'
        '<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="Heading2"/>'
        '<w:lvlText w:val="%1.%2"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="720"/></w:pPr></w:lvl>'
        '<w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="Heading3"/>'
        '<w:lvlText w:val="%1.%2.%3"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="864" w:hanging="864"/></w:pPr></w:lvl>'
    )
    lvls_c = (
        '<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="(%1)"/><w:lvlJc w:val="left"/>'
        '<w:pPr><w:ind w:left="1080" w:hanging="540"/></w:pPr></w:lvl>'
        '<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="(%2)"/><w:lvlJc w:val="left"/>'
        '<w:pPr><w:ind w:left="1620" w:hanging="540"/></w:pPr></w:lvl>'
        '<w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="upperLetter"/><w:lvlText w:val="(%3)"/><w:lvlJc w:val="left"/>'
        '<w:pPr><w:ind w:left="2160" w:hanging="540"/></w:pPr></w:lvl>'
    )
    lvls_b = (
        '<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/>'
        '<w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol"/></w:rPr></w:lvl>'
        '<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/><w:lvlJc w:val="left"/>'
        '<w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl>'
    )
    out = [
        XML,
        f"<w:numbering {W_NS}>",
        f'<w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="multilevel"/>{lvls_h}</w:abstractNum>',
        f'<w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="multilevel"/>{lvls_c}</w:abstractNum>',
        f'<w:abstractNum w:abstractNumId="2"><w:multiLevelType w:val="hybridMultilevel"/>{lvls_b}</w:abstractNum>',
        '<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>',
    ]
    for i in range(n_clause_lists):
        out.append(
            f'<w:num w:numId="{100 + i}"><w:abstractNumId w:val="1"/>'
            '<w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>'
        )
    out.append('<w:num w:numId="2"><w:abstractNumId w:val="2"/></w:num>')
    out.append("</w:numbering>")
    return "".join(out)


def settings_xml(even_odd=False):
    return (
        XML
        + f"<w:settings {W_NS}>"
        + ("<w:evenAndOddHeaders/>" if even_odd else "")
        + '<w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/>'
        '<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>'
        '<w:endnotePr><w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr>'
        '<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat>'
        "</w:settings>"
    )


def notes_xml(kind, notes):
    tag = "footnote" if kind == "footnotes" else "endnote"
    out = [
        XML,
        f"<w:{kind} {W_NS}>",
        f'<w:{tag} w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:{tag}>',
        f'<w:{tag} w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:{tag}>',
    ]
    for nid, text in notes:
        out.append(
            f'<w:{tag} w:id="{nid}"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>'
            f'<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>'
            f"{run(' ' + text)}</w:p></w:{tag}>"
        )
    out.append(f"</w:{kind}>")
    return "".join(out)


def hf_xml(kind, body):
    tag = "hdr" if kind == "header" else "ftr"
    return XML + f"<w:{tag} {W_NS}>{body}</w:{tag}>"


def sect_pr(pkg_refs, landscape=False, title_pg=False, start=None, fmt=None, cont=False, margins=1440):
    w, h = (16838, 11906) if landscape else (11906, 16838)
    refs = "".join(f'<w:{k}Reference w:type="{typ}" r:id="{rid}"/>' for (k, typ, rid) in pkg_refs)
    pg = ""
    if start is not None or fmt:
        pg = "<w:pgNumType" + (f' w:fmt="{fmt}"' if fmt else "") + (f' w:start="{start}"' if start is not None else "") + "/>"
    return (
        f"<w:sectPr>{refs}"
        + ('<w:type w:val="continuous"/>' if cont else "")
        + f'<w:pgSz w:w="{w}" w:h="{h}"' + (' w:orient="landscape"' if landscape else "") + "/>"
        f'<w:pgMar w:top="{margins}" w:right="{margins}" w:bottom="{margins}" w:left="{margins}" w:header="708" w:footer="708" w:gutter="0"/>'
        + pg
        + ("<w:titlePg/>" if title_pg else "")
        + '<w:cols w:space="708"/><w:docGrid w:linePitch="360"/></w:sectPr>'
    )


def table(rng, words, rows, cols, widths, header_rows=1, repeat=True, numeric_from=1):
    grid = "".join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    out = [
        '<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/>'
        '<w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr>'
        f"<w:tblGrid>{grid}</w:tblGrid>"
    ]
    for r in range(rows):
        hdr = r < header_rows
        trpr = "<w:trPr>" + ("<w:tblHeader/>" if hdr and repeat else "") + "<w:cantSplit/></w:trPr>"
        cells = []
        for c in range(cols):
            if hdr:
                txt = " ".join(rng.choice(words) for _ in range(rng.randint(1, 3))).title()
                body = run(txt, "<w:b/>")
                shade = '<w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>'
            elif c >= numeric_from:
                body = run(f"{rng.randint(10, 99999):,}.{rng.randint(0, 99):02d}")
                shade = ""
            else:
                body = run(" ".join(rng.choice(words) for _ in range(rng.randint(2, 9))))
                shade = ""
            jc = '<w:jc w:val="right"/>' if (c >= numeric_from and not hdr) else ""
            cells.append(
                f'<w:tc><w:tcPr><w:tcW w:w="{widths[c]}" w:type="dxa"/>{shade}</w:tcPr>'
                f"<w:p><w:pPr>{jc}</w:pPr>{body}</w:p></w:tc>"
            )
        out.append(f"<w:tr>{trpr}{''.join(cells)}</w:tr>")
    out.append("</w:tbl>")
    return "".join(out)


# ── pictures ────────────────────────────────────────────────────────────────


def png(width, height, rng):
    """A bar-chart-like PNG (stdlib only)."""
    bars = [rng.randint(height // 8, height - 20) for _ in range(12)]
    colors = [(68, 114, 196), (237, 125, 49), (165, 165, 165), (255, 192, 0)]
    bw = width // 12
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        row = bytearray()
        for i, b in enumerate(bars):
            c = colors[i % 4] if height - y <= b else (255, 255, 255)
            px = bytes(c) * (bw - 4) + b"\xff\xff\xff" * 4
            row += px
        row += b"\xff\xff\xff" * (width - len(row) // 3)
        if y % 40 == 0:
            row = bytearray(b"\xd0\xd0\xd0" * width)
        raw += row
    def chunk(k, d):
        return struct.pack(">I", len(d)) + k + d + struct.pack(">I", zlib.crc32(k + d) & 0xFFFFFFFF)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
        + chunk(b"IEND", b"")
    )


def photo(cache_dir, i, width, height):
    """A JPEG 'photo' (ImageMagick plasma with a fixed seed), cached."""
    path = os.path.join(cache_dir, f"photo{i:03d}_{width}x{height}.jpeg")
    if not os.path.exists(path):
        subprocess.run(
            ["magick", "-seed", str(1000 + i), "-size", f"{width}x{height}", "plasma:",
             "-blur", "0x1", "-quality", "85", "-strip", path],
            check=True,
        )
    with open(path, "rb") as f:
        return f.read()


def drawing_inline(rid, pid, cx, cy, name):
    return (
        f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>'
        f'<wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{pid}" name="{name}"/>'
        '<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>'
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{pid}" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr>'
        f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>'
        "</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )


def drawing_anchor(rid, pid, cx, cy, name, right=True, z=0):
    h = '<wp:positionH relativeFrom="column"><wp:align>' + ("right" if right else "left") + "</wp:align></wp:positionH>"
    return (
        f'<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" '
        f'relativeHeight="{251659264 + z}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1">'
        '<wp:simplePos x="0" y="0"/>' + h
        + '<wp:positionV relativeFrom="paragraph"><wp:posOffset>63500</wp:posOffset></wp:positionV>'
        f'<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapSquare wrapText="bothSides"/>'
        f'<wp:docPr id="{pid}" name="{name}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>'
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{pid}" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr>'
        f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>'
        "</pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"
    )


def mark_spots(body):
    """Words to find the middle and the end of the document by (Find in the
    app, to put the caret there): a paragraph in the middle and one near
    the end, before the last section break."""
    mid = len(body) // 2
    while body[mid].startswith("<w:tbl>") or "<w:sectPr>" in body[mid]:
        mid += 1
    body.insert(mid, para(run("Checkpoint MIDPOINTMARK in the middle of the document.")))
    end = len(body) - 3
    while body[end].startswith("<w:tbl>") or "<w:sectPr>" in body[end]:
        end -= 1
    body.insert(end, para(run("Checkpoint ENDPOINTMARK near the end of the document.")))


# ── contract / regulation compilation ───────────────────────────────────────


class Review:
    """Tracked changes and comments sprinkled over a document."""

    AUTHORS = ["Rina Hartono", "James Whitfield", "Legal Review"]

    def __init__(self, rng, on):
        self.rng = rng
        self.on = on
        self.next_id = 1
        self.comments = []  # (id, author, text, parent_id or None)
        self.counts = {"ins": 0, "del": 0, "fmt": 0, "comments": 0}

    def rid(self):
        i = self.next_id
        self.next_id += 1
        return i

    def text_runs(self, text, words):
        """A body paragraph's runs, maybe with review marks."""
        rng = self.rng
        if not self.on or rng.random() > 0.22:
            return run(text)
        sents = text.split(". ")
        out = []
        for i, s in enumerate(sents):
            s = s + (". " if i < len(sents) - 1 else "")
            roll = rng.random()
            author = rng.choice(self.AUTHORS)
            if roll < 0.25:
                out.append(f'<w:del w:id="{self.rid()}" w:author="{author}" w:date="{DATE}"><w:r><w:delText xml:space="preserve">{escape(s)}</w:delText></w:r></w:del>')
                self.counts["del"] += 1
            elif roll < 0.5:
                out.append(run(s))
                ins = " " + sentence(rng, words, 5, 14) + " "
                out.append(f'<w:ins w:id="{self.rid()}" w:author="{author}" w:date="{DATE}">{run(ins)}</w:ins>')
                self.counts["ins"] += 1
            elif roll < 0.62:
                out.append(
                    f'<w:r><w:rPr><w:b/><w:rPrChange w:id="{self.rid()}" w:author="{author}" w:date="{DATE}"><w:rPr/></w:rPrChange></w:rPr>{t(s)}</w:r>'
                )
                self.counts["fmt"] += 1
            elif roll < 0.85:
                cid = len(self.comments)
                self.comments.append((cid, author, sentence(rng, words, 6, 18), None))
                if rng.random() < 0.4:
                    other = rng.choice([a for a in self.AUTHORS if a != author])
                    self.comments.append((len(self.comments), other, sentence(rng, words, 4, 12), cid))
                self.counts["comments"] += 1
                out.append(
                    f'<w:commentRangeStart w:id="{cid}"/>{run(s)}<w:commentRangeEnd w:id="{cid}"/>'
                    f'<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{cid}"/></w:r>'
                )
                # Replies are anchored on the same range in Word.
                for c in self.comments[cid + 1:]:
                    out.append(
                        f'<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{c[0]}"/></w:r>'
                    )
            else:
                out.append(run(s))
        return "".join(out)

    def comments_xml(self):
        out = [XML, f"<w:comments {W_NS}>"]
        for cid, author, text, _ in self.comments:
            pid = f"{0x10000000 + cid:08X}"
            initials = "".join(w[0] for w in author.split())
            out.append(
                f'<w:comment w:id="{cid}" w:author="{escape(author)}" w:date="{DATE}" w:initials="{initials}">'
                f'<w:p w14:paraId="{pid}" w14:textId="77777777"><w:pPr><w:pStyle w:val="CommentText"/></w:pPr>'
                f'<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>{run(text)}</w:p></w:comment>'
            )
        out.append("</w:comments>")
        return "".join(out)

    def comments_ext_xml(self):
        out = [XML, f"<w15:commentsEx {W_NS}>"]
        for cid, _, _, parent in self.comments:
            pid = f"{0x10000000 + cid:08X}"
            p = f' w15:paraIdParent="{0x10000000 + parent:08X}"' if parent is not None else ""
            out.append(f'<w15:commentEx w15:paraId="{pid}"{p} w15:done="0"/>')
        out.append("</w15:commentsEx>")
        return "".join(out)


def contract(path, scale, seed, reviewed):
    rng = random.Random(seed)
    words = WORDS
    pkg = Pkg()
    review = Review(random.Random(seed + 7), reviewed)

    pkg.add_part("word/styles.xml", styles_xml(), "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml", "styles")
    pkg.add_part("word/settings.xml", settings_xml(), "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml", "settings")

    # Headers and footers: cover (first page, empty), running "Page X of Y".
    def hf(kind, name, body):
        return pkg.add_part(
            f"word/{name}.xml",
            hf_xml(kind, body),
            f"application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml",
            kind,
        )

    title = "Consolidated Regulations and Conditions of Contract"
    h_first = hf("header", "header1", para("", "Header"))
    f_first = hf("footer", "footer1", para(run("CONFIDENTIAL — FOR CONSULTATION"), "Footer", '<w:jc w:val="center"/>'))
    h_main = hf("header", "header2", para(run(title) + run("\t") + run("\tVersion 4.2"), "Header", '<w:pBdr><w:bottom w:val="single" w:sz="4" w:space="1" w:color="auto"/></w:pBdr>'))
    f_main = hf(
        "footer",
        "footer2",
        para(run("Page ") + field("PAGE \\* MERGEFORMAT", "1") + run(" of ") + field("NUMPAGES \\* MERGEFORMAT", "1"), "Footer", '<w:jc w:val="right"/>'),
    )
    h_land = hf("header", "header3", para(run(title + " — Schedules"), "Header"))
    f_land = hf(
        "footer",
        "footer3",
        para(run("Schedule page ") + field("PAGE", "1") + run(" of ") + field("SECTIONPAGES", "1") + run("\tPage ") + field("PAGE", "1") + run(" of ") + field("NUMPAGES", "1"), "Footer"),
    )
    cover_refs = [("header", "first", h_first), ("footer", "first", f_first), ("header", "default", h_main), ("footer", "default", f_main)]
    main_refs = [("header", "default", h_main), ("footer", "default", f_main)]
    land_refs = [("header", "default", h_land), ("footer", "default", f_land)]

    n_articles = max(4, int(90 * scale))
    parts = 5
    body = []
    toc = []  # (level, number, text, bookmark)
    bm = 0
    clause_lists = 0
    headings = []  # (number, bookmark) for cross references

    def heading(level, number, text):
        nonlocal bm
        bm += 1
        name = f"_Ref{100000 + bm}"
        toc.append((level, number, text, name))
        headings.append((number, name))
        return (
            f'<w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr>'
            f'<w:bookmarkStart w:id="{bm}" w:name="{name}"/>{run(text)}<w:bookmarkEnd w:id="{bm}"/></w:p>'
        )

    def body_para():
        text = para_text(rng, words)
        extra = ""
        if headings and rng.random() < 0.18:
            num, ref = rng.choice(headings[-60:])
            extra = run(" As provided in ") + field(f"REF {ref} \\r \\h", num) + run(" (see page ") + field(f"PAGEREF {ref} \\h", str(rng.randint(5, 900))) + run("), the same applies.")
        return para(review.text_runs(text, words) + extra)

    per_part = n_articles // parts
    art = 0
    for part_i in range(parts):
        landscape_after = part_i in (1, 3)
        for _ in range(per_part):
            art += 1
            body.append(heading(1, f"Article {art}", " ".join(rng.choice(words) for _ in range(rng.randint(2, 5))).title()))
            for s in range(1, rng.randint(5, 8)):
                body.append(heading(2, f"{art}.{s}", " ".join(rng.choice(words) for _ in range(rng.randint(2, 6))).title()))
                body.append(body_para())
                for ss in range(1, rng.randint(3, 6)):
                    body.append(heading(3, f"{art}.{s}.{ss}", " ".join(rng.choice(words) for _ in range(rng.randint(3, 7))).title()))
                    for _ in range(rng.randint(1, 3)):
                        body.append(body_para())
                    if rng.random() < 0.55:
                        num_id = 100 + clause_lists
                        clause_lists += 1
                        for _ in range(rng.randint(2, 5)):
                            body.append(para(review.text_runs(sentence(rng, words, 12, 40), words), "ListParagraph", f'<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num_id}"/></w:numPr>'))
                            if rng.random() < 0.4:
                                for _ in range(rng.randint(1, 3)):
                                    body.append(para(review.text_runs(sentence(rng, words, 8, 30), words), "ListParagraph", f'<w:numPr><w:ilvl w:val="1"/><w:numId w:val="{num_id}"/></w:numPr>'))
                    if rng.random() < 0.08:
                        body.append(table(rng, words, rng.randint(6, 14), 4, [3200, 2000, 2000, 2150]))
                        body.append(para(""))
        # End of the part's portrait section.
        refs = cover_refs if part_i == 0 else main_refs
        body.append(para(run(f"End of Part {part_i + 1}."), None, sect_pr(refs, title_pg=part_i == 0)))
        if landscape_after:
            body.append(para(run(f"Schedule {part_i + 1}: Rates and Quantities"), "Heading1", '<w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr>'))
            for _ in range(int(6 * max(scale, 0.2)) + 1):
                body.append(table(rng, words, 40, 8, [3000, 1500, 1500, 1500, 1500, 1500, 1500, 1830]))
                body.append(para(""))
            body.append(para(run("End of schedule."), None, sect_pr(land_refs, landscape=True)))

    mark_spots(body)

    # Front matter: cover, table of contents.
    front = [
        para(run(title), "Title"),
        para(run("Compiled edition incorporating all amendments"), "Subtitle"),
        para(run("Issued 1 October 2026"), "Subtitle", '<w:spacing w:before="1200"/>'),
        para('<w:r><w:br w:type="page"/></w:r>'),
        para(run("Contents"), "TOCHeading"),
    ]
    toc_paras = []
    for i, (level, number, text, name) in enumerate(toc):
        page = 3 + i * 900 // max(1, len(toc))
        inner = (
            f'<w:hyperlink w:anchor="{name}" w:history="1">{run(number + " " + text)}{run(chr(9))}'
            + field(f"PAGEREF {name} \\h", str(page))
            + "</w:hyperlink>"
        )
        toc_paras.append((level, inner))
    begin = '<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \\o "1-3" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>'
    for i, (level, inner) in enumerate(toc_paras):
        pre = begin if i == 0 else ""
        post = '<w:r><w:fldChar w:fldCharType="end"/></w:r>' if i == len(toc_paras) - 1 else ""
        front.append(para(pre + inner + post, f"TOC{level}"))
    front.append(para('<w:r><w:br w:type="page"/></w:r>'))

    final = sect_pr(main_refs)
    doc = f"{XML}<w:document {W_NS}><w:body>{''.join(front)}{''.join(body)}{final}</w:body></w:document>"
    pkg.add_part("word/numbering.xml", numbering_xml(clause_lists), "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml", "numbering")
    if reviewed:
        pkg.add_part("word/comments.xml", review.comments_xml(), "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml", "comments")
        pkg.add_part("word/commentsExtended.xml", review.comments_ext_xml(), "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml", "http://schemas.microsoft.com/office/2011/relationships/commentsExtended")
    write_pkg(pkg, path, doc, title)
    return {"paragraphs": len(front) + len(body), "articles": art, "toc": len(toc), **(review.counts if reviewed else {})}


def write_pkg(pkg, path, doc, title):
    pkg.write(path, doc, title)


# ── thesis / manual ─────────────────────────────────────────────────────────


def thesis(path, scale, seed, cache_dir):
    rng = random.Random(seed)
    words = THESIS_WORDS
    pkg = Pkg()
    pkg.add_part("word/styles.xml", styles_xml("Georgia", "Times New Roman"), "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml", "styles")
    pkg.add_part("word/settings.xml", settings_xml(), "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml", "settings")

    def hf(kind, name, body):
        return pkg.add_part(f"word/{name}.xml", hf_xml(kind, body), f"application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml", kind)

    title = "Thermal Behaviour of Industrial Process Systems: Measurement, Modelling and Control"
    h_front = hf("header", "header1", para("", "Header"))
    f_front = hf("footer", "footer1", para(field("PAGE \\* roman", "i"), "Footer", '<w:jc w:val="center"/>'))
    h_body = hf("header", "header2", para(field('STYLEREF "Heading 1" \\n', "1") + run(" ") + field('STYLEREF "Heading 1"', "Introduction") + run("\t\t") + run("Doctoral Thesis"), "Header", '<w:pBdr><w:bottom w:val="single" w:sz="4" w:space="1" w:color="auto"/></w:pBdr>'))
    f_body = hf("footer", "footer2", para(field("PAGE", "1"), "Footer", '<w:jc w:val="center"/>'))
    h_chap = hf("header", "header3", para("", "Header"))

    # Pictures: JPEG photos (ImageMagick) or PNG charts.
    have_magick = shutil.which("magick") is not None
    n_photos = max(8, int(110 * scale))
    photos = []
    for i in range(n_photos):
        if have_magick and i % 4 != 3:
            w, h = (1600, 1067) if i % 2 == 0 else (1200, 900)
            data = photo(cache_dir, i, w, h)
            name = f"image{i + 1}.jpeg"
        else:
            w, h = 900, 560
            data = png(w, h, random.Random(seed * 31 + i))
            name = f"image{i + 1}.png"
        pkg.media[name] = data
        rid = pkg.rel("image", "media/" + name)
        photos.append((rid, w, h))

    notes = []
    body = []
    pid = 0
    fig = 0
    tab = 0

    def note_ref():
        nid = len(notes) + 1
        notes.append((nid, sentence(rng, words, 8, 30)))
        return f'<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="{nid}"/></w:r>'

    def text_para():
        nonlocal pid
        parts = []
        for s in range(rng.randint(3, 6)):
            parts.append(run(sentence(rng, words) + " "))
            if rng.random() < 0.09:
                parts.append(note_ref())
        if rng.random() < 0.05:
            pid += 1
            rid, w, h = rng.choice(photos)
            cx = 2286000
            parts.insert(0, drawing_anchor(rid, pid, cx, cx * h // w, f"Picture {pid}", right=rng.random() < 0.5, z=pid))
        return para("".join(parts), None, '<w:ind w:firstLine="432"/>')

    def figure():
        nonlocal pid, fig
        pid += 1
        fig += 1
        rid, w, h = photos[(fig * 7) % len(photos)]
        cx = 5486400 if w > h * 1.2 else 4572000
        cy = cx * h // w
        return [
            para(drawing_inline(rid, pid, cx, cy, f"Picture {pid}"), None, '<w:keepNext/><w:jc w:val="center"/><w:spacing w:after="60"/>'),
            para(run("Figure ") + field("SEQ Figure \\* ARABIC", str(fig)) + run(". " + sentence(rng, words, 6, 16)), "Caption"),
        ]

    def long_table():
        nonlocal tab
        tab += 1
        out = [para(run("Table ") + field("SEQ Table \\* ARABIC", str(tab)) + run(". " + sentence(rng, words, 5, 12)), "Caption", "<w:keepNext/>")]
        out.append(table(rng, words, rng.randint(45, 90), 5, [2800, 1650, 1650, 1650, 1600], header_rows=1))
        out.append(para(""))
        return out

    # Front matter (roman pages): title, abstract, acknowledgements, TOC.
    front = [para(run(title), "Title"), para(run("A thesis submitted for the degree of Doctor of Philosophy"), "Subtitle"), para('<w:r><w:br w:type="page"/></w:r>')]
    for head in ("Abstract", "Acknowledgements"):
        front.append(para(run(head), "TOCHeading"))
        for _ in range(6):
            front.append(text_para())
        front.append(para('<w:r><w:br w:type="page"/></w:r>'))
    front.append(para(run("Table of Contents"), "TOCHeading"))
    toc_at = len(front)
    front.append(None)  # replaced below
    front.append(para(run(""), None, sect_pr([("header", "default", h_front), ("footer", "default", f_front)], fmt="lowerRoman", start=1)))

    chapters = max(3, int(18 * scale))
    toc = []
    bm = 0
    for c in range(1, chapters + 1):
        bm += 1
        name = f"_Toc{200000 + bm}"
        ctitle = " ".join(rng.choice(words) for _ in range(rng.randint(2, 5))).title()
        toc.append((1, f"{c}", ctitle, name))
        body.append(f'<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:pageBreakBefore/></w:pPr><w:bookmarkStart w:id="{bm}" w:name="{name}"/>{run(ctitle)}<w:bookmarkEnd w:id="{bm}"/></w:p>')
        for s in range(1, rng.randint(6, 9)):
            bm += 1
            name = f"_Toc{200000 + bm}"
            stitle = " ".join(rng.choice(words) for _ in range(rng.randint(3, 7))).title()
            toc.append((2, f"{c}.{s}", stitle, name))
            body.append(f'<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:bookmarkStart w:id="{bm}" w:name="{name}"/>{run(stitle)}<w:bookmarkEnd w:id="{bm}"/></w:p>')
            for ss in range(1, rng.randint(3, 6)):
                bm += 1
                name = f"_Toc{200000 + bm}"
                sstitle = " ".join(rng.choice(words) for _ in range(rng.randint(3, 7))).title()
                toc.append((3, f"{c}.{s}.{ss}", sstitle, name))
                body.append(f'<w:p><w:pPr><w:pStyle w:val="Heading3"/></w:pPr><w:bookmarkStart w:id="{bm}" w:name="{name}"/>{run(sstitle)}<w:bookmarkEnd w:id="{bm}"/></w:p>')
                for k in range(rng.randint(5, 9)):
                    body.append(text_para())
                    if k == 2 and rng.random() < 0.85:
                        body.extend(figure())
                    if rng.random() < 0.04:
                        for _ in range(rng.randint(3, 6)):
                            body.append(para(run(sentence(rng, words, 6, 20)), "ListParagraph", '<w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr>'))
                if rng.random() < 0.22:
                    body.extend(long_table())
    # References chapter.
    body.append(para(run("References"), "Heading1", '<w:pageBreakBefore/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr>'))
    for i in range(int(400 * scale) + 20):
        body.append(para(run(f"[{i + 1}] " + sentence(rng, words, 10, 22)), None, '<w:ind w:left="567" w:hanging="567"/><w:jc w:val="left"/>'))

    mark_spots(body)

    toc_xml = []
    begin = '<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \\o "1-3" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>'
    for i, (level, number, text, name) in enumerate(toc):
        page = 1 + i * 1100 // max(1, len(toc))
        inner = f'<w:hyperlink w:anchor="{name}" w:history="1">{run(number + " " + text)}{run(chr(9))}{field(f"PAGEREF {name} \\h", str(page))}</w:hyperlink>'
        pre = begin if i == 0 else ""
        post = '<w:r><w:fldChar w:fldCharType="end"/></w:r>' if i == len(toc) - 1 else ""
        toc_xml.append(para(pre + inner + post, f"TOC{level}"))
    front[toc_at] = "".join(toc_xml)

    pkg.add_part("word/numbering.xml", numbering_xml(0), "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml", "numbering")
    pkg.add_part("word/footnotes.xml", notes_xml("footnotes", notes), "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml", "footnotes")
    pkg.add_part("word/endnotes.xml", notes_xml("endnotes", []), "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml", "endnotes")
    final = sect_pr([("header", "default", h_body), ("footer", "default", f_body), ("header", "first", h_chap), ("footer", "first", f_body)], start=1)
    doc = f"{XML}<w:document {W_NS}><w:body>{''.join(front)}{''.join(body)}{final}</w:body></w:document>"
    write_pkg(pkg, path, doc, title)
    return {"paragraphs": len(body), "figures": fig, "tables": tab, "footnotes": len(notes), "floats": pid - fig, "pictures": len(photos)}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("outdir", nargs="?", default=os.path.expanduser("~/.cache/bigdoc"))
    ap.add_argument("--scale", type=float, default=1.0, help="size factor (1.0 = ~1,000-1,300 pages each)")
    ap.add_argument("--only", choices=["contract", "thesis", "reviewed"], action="append")
    ap.add_argument("--seed", type=int, default=20261127)
    a = ap.parse_args()
    os.makedirs(a.outdir, exist_ok=True)
    cache = os.path.join(a.outdir, "pictures")
    os.makedirs(cache, exist_ok=True)
    only = a.only or ["contract", "thesis", "reviewed"]
    if "contract" in only:
        print("contract.docx", contract(os.path.join(a.outdir, "contract.docx"), a.scale, a.seed, False))
    if "reviewed" in only:
        print("reviewed.docx", contract(os.path.join(a.outdir, "reviewed.docx"), a.scale, a.seed, True))
    if "thesis" in only:
        print("thesis.docx", thesis(os.path.join(a.outdir, "thesis.docx"), a.scale, a.seed + 1, cache))


if __name__ == "__main__":
    sys.exit(main())
