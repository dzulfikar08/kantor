#!/usr/bin/env python3
"""Write the legacy Word 97-2003 (.doc) fixtures (CC0, synthetic):
tests/corpus/doc/*.doc, read by crates/docs_engine/tests/msdoc.rs.

Each file is written as .docx with python-docx from made-up text, then
converted to .doc by LibreOffice ("MS Word 97" filter):

* text.doc: character formatting (bold, italic, underline, strike, size,
  font, colour, highlight, super/subscript), headings 1-3, alignment,
  indents, spacing, line spacing, a tab, a line break, a page break,
  Indonesian, CJK and emoji text, a HYPERLINK field.
* tables.doc: a table with set column widths, a horizontally and a
  vertically merged cell, bold header row; text after the table.
* lists.doc: a bulleted list, a numbered list with a second level.
* page.doc: US Letter landscape with custom margins, a header and a footer
  with a PAGE field, then a portrait A4 section.
* picture.doc: an inline PNG picture between two paragraphs.
* comments.doc: three comments by two authors: on a word, on a span
  crossing two runs (with two lines of text) and on a whole paragraph.
* floats.doc: tests/corpus/docx/floats.docx (floating pictures, shapes and
  text boxes, from gen_docx.py) as LibreOffice saves it in Word 97 format
  (its DrawingML shapes become freeforms drawn with guide formulas).
* shapes.doc: a flat ODT (SHAPES_FODT below) with LibreOffice's own shapes,
  which it saves as Word's preset types: a rectangle with text, an ellipse
  behind the text, a rounded rectangle placed from the page, a line with an
  arrowhead and a text frame with two paragraphs.

Regenerate with
`uv run --with python-docx==1.2.0 scripts/corpus/gen_doc.py tests/corpus/doc [soffice]`
where soffice defaults to ~/.cache/lo/lo.AppImage (LibreOffice 25.8).
"""
import datetime
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

from docx import Document
from docx.enum.section import WD_ORIENT, WD_SECTION
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK, WD_COLOR_INDEX
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Cm, Pt, RGBColor

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/doc").resolve()
soffice = Path(sys.argv[2] if len(sys.argv) > 2 else Path.home() / ".cache/lo/lo.AppImage")
FIXED = datetime.datetime(2026, 11, 13, 0, 0, 0)


def field(par, instr, result):
    """Append a complex field to a paragraph."""
    for kind in ("begin", "instr", "separate", "text", "end"):
        r = OxmlElement("w:r")
        if kind == "instr":
            t = OxmlElement("w:instrText")
            t.set(qn("xml:space"), "preserve")
            t.text = f" {instr} "
            r.append(t)
        elif kind == "text":
            t = OxmlElement("w:t")
            t.text = result
            r.append(t)
        else:
            fc = OxmlElement("w:fldChar")
            fc.set(qn("w:fldCharType"), kind)
            r.append(fc)
        par._p.append(r)


def text_doc():
    d = Document()
    d.add_heading("Laporan Kantor", level=1)
    d.add_heading("Ringkasan", level=2)
    d.add_heading("Detail", level=3)
    p = d.add_paragraph("Plain, ")
    p.add_run("bold").bold = True
    p.add_run(", ")
    p.add_run("italic").italic = True
    p.add_run(", ")
    p.add_run("underlined").underline = True
    p.add_run(", ")
    p.add_run("struck").font.strike = True
    p.add_run(", ")
    r = p.add_run("big red")
    r.font.size = Pt(18)
    r.font.color.rgb = RGBColor(0xC0, 0x00, 0x00)
    p.add_run(", ")
    r = p.add_run("Courier New")
    r.font.name = "Courier New"
    p.add_run(", ")
    r = p.add_run("highlighted")
    r.font.highlight_color = WD_COLOR_INDEX.YELLOW
    p.add_run(", E=mc")
    p.add_run("2").font.superscript = True
    p.add_run(", H")
    p.add_run("2").font.subscript = True
    p.add_run("O.")
    p = d.add_paragraph("Centred paragraph.")
    p.alignment = WD_ALIGN_PARAGRAPH.CENTER
    p = d.add_paragraph("Right aligned paragraph.")
    p.alignment = WD_ALIGN_PARAGRAPH.RIGHT
    p = d.add_paragraph(
        "Justified and indented paragraph with space before and after and one and a half "
        "line spacing, long enough to wrap onto a second line on an A4 page."
    )
    p.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
    f = p.paragraph_format
    f.left_indent = Cm(2)
    f.right_indent = Cm(1)
    f.first_line_indent = Cm(1)
    f.space_before = Pt(12)
    f.space_after = Pt(6)
    f.line_spacing = 1.5
    p = d.add_paragraph("Name:\tKantor")
    p.add_run().add_break(WD_BREAK.LINE)
    p.add_run("Second line")
    d.add_paragraph("Bahasa Indonesia: Selamat pagi, apa kabar? Rp 1.250.000,00")
    d.add_paragraph("CJK: 你好世界 こんにちは 안녕하세요")
    d.add_paragraph("Emoji: 😀 👍🏽 ✓")
    p = d.add_paragraph("Visit ")
    field(p, 'HYPERLINK "https://kantor.paparanai.id/"', "the Kantor site")
    p.add_run(" today.")
    p = d.add_paragraph("Before the break")
    p.add_run().add_break(WD_BREAK.PAGE)
    p.add_run("After the break")
    d.add_paragraph("Last paragraph.")
    return d


def tables_doc():
    d = Document()
    d.add_paragraph("Quarterly figures")
    t = d.add_table(rows=4, cols=3)
    t.style = "Table Grid"
    widths = [Cm(6), Cm(4), Cm(4)]
    data = [
        ["Item", "Q1", "Q2"],
        ["Paper", "120", "135"],
        ["Ink", "40", "38"],
        ["Total", "", ""],
    ]
    for i, row in enumerate(data):
        for j, v in enumerate(row):
            c = t.cell(i, j)
            c.width = widths[j]
            c.text = v
            if i == 0:
                c.paragraphs[0].runs[0].bold = True
    a = t.cell(3, 1).merge(t.cell(3, 2))
    a.text = "313"
    b = t.cell(1, 0).merge(t.cell(2, 0))
    b.text = "Supplies"
    d.add_paragraph("Text after the table.")
    return d


def lists_doc():
    d = Document()
    d.add_paragraph("Shopping list")
    for s in ["Apples", "Bread", "Coffee"]:
        d.add_paragraph(s, style="List Bullet")
    d.add_paragraph("Steps")
    d.add_paragraph("Open the file", style="List Number")
    d.add_paragraph("Check the text", style="List Number")
    d.add_paragraph("Headings", style="List Number 2")
    d.add_paragraph("Save as .docx", style="List Number")
    d.add_paragraph("Done.")
    return d


def page_doc():
    d = Document()
    s = d.sections[0]
    s.orientation = WD_ORIENT.LANDSCAPE
    s.page_width = Cm(27.94)
    s.page_height = Cm(21.59)
    s.left_margin = Cm(3)
    s.right_margin = Cm(2)
    s.top_margin = Cm(2.5)
    s.bottom_margin = Cm(1.5)
    s.header.paragraphs[0].text = "Kantor header"
    fp = s.footer.paragraphs[0]
    fp.text = "Page "
    field(fp, "PAGE", "1")
    d.add_paragraph("Landscape letter page.")
    s2 = d.add_section(WD_SECTION.NEW_PAGE)
    s2.orientation = WD_ORIENT.PORTRAIT
    s2.page_width = Cm(21.0)
    s2.page_height = Cm(29.7)
    s2.left_margin = s2.right_margin = Cm(2.54)
    s2.top_margin = s2.bottom_margin = Cm(2.54)
    d.add_paragraph("Portrait A4 page.")
    return d


def png(w, h):
    """A small RGB PNG: a blue-to-green gradient."""
    raw = b"".join(
        b"\x00" + b"".join(bytes((0, 255 * x // w, 255 * y // h)) for x in range(w))
        for y in range(h)
    )

    def chunk(t, data):
        c = struct.pack(">I", len(data)) + t + data
        return c + struct.pack(">I", zlib.crc32(t + data))

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def picture_doc(tmp):
    d = Document()
    d.add_paragraph("Above the picture.")
    img = tmp / "gradient.png"
    img.write_bytes(png(64, 32))
    d.add_paragraph().add_run().add_picture(str(img), width=Cm(6))
    d.add_paragraph("Below the picture.")
    return d


SHAPES_FODT = """<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">
 <office:styles>
  <draw:marker draw:name="Arrow" svg:viewBox="0 0 20 30" svg:d="M10 0l-10 30h20z"/>
 </office:styles>
 <office:automatic-styles>
  <style:style style:name="blue" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#4472c4" draw:stroke="solid" svg:stroke-color="#1f3864" svg:stroke-width="0.05cm" style:wrap="parallel" style:vertical-pos="from-top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="paragraph"/></style:style>
  <style:style style:name="behind" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#ddebff" draw:stroke="solid" svg:stroke-color="#4472c4" style:wrap="run-through" style:run-through="background" style:vertical-pos="from-top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="paragraph"/></style:style>
  <style:style style:name="round" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#fff4d6" draw:stroke="solid" svg:stroke-color="#c08a00" style:wrap="none" style:vertical-pos="from-top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="page"/></style:style>
  <style:style style:name="arrow" style:family="graphic"><style:graphic-properties draw:stroke="solid" svg:stroke-color="#c00000" svg:stroke-width="0.05cm" draw:marker-end="Arrow" draw:marker-end-width="0.3cm" style:wrap="run-through" style:run-through="foreground" style:vertical-pos="from-top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="paragraph"/></style:style>
  <style:style style:name="frame" style:family="graphic"><style:graphic-properties fo:border="0.05cm solid #000000" fo:background-color="#ffffff" style:wrap="dynamic" style:vertical-pos="from-top" style:vertical-rel="paragraph" style:horizontal-pos="from-left" style:horizontal-rel="paragraph"/></style:style>
 </office:automatic-styles>
 <office:body><office:text>
  <text:h text:outline-level="1">Shapes</text:h>
  <text:p><draw:rect draw:name="Blue box" draw:style-name="blue" text:anchor-type="paragraph" svg:x="11cm" svg:y="0cm" svg:width="4cm" svg:height="2cm"><text:p>Revenue</text:p></draw:rect>Kantor reads floating shapes from Word 97-2003 files: rectangles, ellipses, rounded rectangles, lines with arrowheads and text boxes, anchored to a paragraph and placed from the page or the paragraph, with the text wrapping around them. Kantor reads floating shapes from Word 97-2003 files.</text:p>
  <text:p><draw:ellipse draw:name="Ellipse behind" draw:style-name="behind" text:anchor-type="paragraph" svg:x="2cm" svg:y="0.2cm" svg:width="8cm" svg:height="1.5cm"/><draw:custom-shape draw:name="Rounded" draw:style-name="round" text:anchor-type="paragraph" svg:x="13cm" svg:y="2.5cm" svg:width="5cm" svg:height="2cm"><text:p>Rounded</text:p><draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:type="round-rectangle" draw:modifiers="3600" draw:enhanced-path="M ?f7 0 X 0 ?f8 L 0 ?f9 Y ?f7 21600 L ?f10 21600 X 21600 ?f9 L 21600 ?f8 Y ?f10 0 Z N"><draw:equation draw:name="f0" draw:formula="45"/><draw:equation draw:name="f1" draw:formula="$0 *sin(?f0 *(pi/180))"/><draw:equation draw:name="f2" draw:formula="?f1 *3163/7636"/><draw:equation draw:name="f3" draw:formula="left+?f2 "/><draw:equation draw:name="f4" draw:formula="top+?f2 "/><draw:equation draw:name="f5" draw:formula="right-?f2 "/><draw:equation draw:name="f6" draw:formula="bottom-?f2 "/><draw:equation draw:name="f7" draw:formula="left+$0 "/><draw:equation draw:name="f8" draw:formula="top+$0 "/><draw:equation draw:name="f9" draw:formula="bottom-$0 "/><draw:equation draw:name="f10" draw:formula="right-$0 "/><draw:handle draw:handle-position="$0 top" draw:handle-switched="true" draw:handle-range-x-minimum="0" draw:handle-range-x-maximum="10800"/></draw:enhanced-geometry></draw:custom-shape>The ellipse sits behind this text and the rounded rectangle is placed from the page. Kantor reads floating shapes from Word 97-2003 files, with the text wrapping around them.</text:p>
  <text:p><draw:line draw:name="Arrow" draw:style-name="arrow" text:anchor-type="paragraph" svg:x1="1cm" svg:y1="0.5cm" svg:x2="6cm" svg:y2="1.5cm"/><draw:frame draw:name="Notes" draw:style-name="frame" text:anchor-type="paragraph" svg:x="8cm" svg:y="0.2cm" svg:width="6cm" svg:height="2cm"><draw:text-box><text:p>A text frame</text:p><text:p>with two lines</text:p></draw:text-box></draw:frame>An arrow in front of the text and a text frame with two paragraphs. Kantor reads floating shapes from Word 97-2003 files, with the text wrapping around them.</text:p>
  <text:p>The end.</text:p>
 </office:text></office:body>
</office:document>
"""


def comments_doc():
    d = Document()
    p = d.add_paragraph("The ")
    word = p.add_run("budget")
    p.add_run(" needs a second look before Friday.")
    d.add_comment(word, text="Check the totals.", author="Ani Wijaya", initials="AW")
    p = d.add_paragraph()
    a = p.add_run("Bold start ")
    a.bold = True
    b = p.add_run("and plain end.")
    d.add_comment([a, b], text="Two runs.\nSecond line.", author="Budi Santoso", initials="BS")
    p = d.add_paragraph("Last paragraph.")
    d.add_comment(p.runs[0], text="On a whole paragraph.", author="Ani Wijaya", initials="AW")
    return d


def main():
    out.mkdir(parents=True, exist_ok=True)
    base = Path(os.environ.get("TMPDIR", Path.home() / ".cache/ktmp"))
    base.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=base) as t:
        tmp = Path(t)
        docs = {
            "text": text_doc(),
            "tables": tables_doc(),
            "lists": lists_doc(),
            "page": page_doc(),
            "picture": picture_doc(tmp),
            "comments": comments_doc(),
        }
        for name, d in docs.items():
            d.core_properties.created = FIXED
            d.core_properties.modified = FIXED
            d.core_properties.author = "Kantor corpus"
            d.core_properties.last_modified_by = "Kantor corpus"
            d.save(tmp / f"{name}.docx")
        shutil.copy(Path(__file__).resolve().parents[2] / "tests/corpus/docx/floats.docx", tmp)
        (tmp / "shapes.fodt").write_text(SHAPES_FODT, encoding="utf-8")
        names = [*docs, "floats"]
        profile = tmp / "profile"
        # 81: LibreOffice made a new profile and wants a restart; run again.
        cmd = [
            str(soffice),
            f"-env:UserInstallation=file://{profile}",
            "--headless",
            "--convert-to",
            "doc:MS Word 97",
            "--outdir",
            str(tmp),
            *[str(tmp / f"{n}.docx") for n in names],
            str(tmp / "shapes.fodt"),
        ]
        if subprocess.run(cmd).returncode == 81:
            subprocess.run(cmd, check=True)
        for n in [*names, "shapes"]:
            shutil.copy(tmp / f"{n}.doc", out / f"{n}.doc")
            print(out / f"{n}.doc")


main()
