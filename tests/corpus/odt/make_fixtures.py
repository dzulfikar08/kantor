#!/usr/bin/env python3
"""Write the OpenDocument text fixtures (CC0, synthetic): tests/corpus/odt/,
read by crates/docs_engine/tests/odt.rs.

Each document is written below as flat ODF XML (.fodt) from made-up text,
then saved as .odt by LibreOffice ("writer8"), so the packages are laid out
as LibreOffice writes them (its own automatic styles, settings, manifest):

* text.odt: named paragraph styles (a custom one based on Text Body),
  headings 1-3, character formatting through automatic and named text
  styles (bold, italic, underline, double underline, strike, size, font,
  colour, highlight, super/subscript, small caps, letter spacing), a named
  character style, alignment, indents, spacing, line spacing, tab stops
  with a dot leader, a line break, runs of spaces, a page break, Indonesian,
  CJK and emoji text, a web link and a link to a bookmark.
* lists.odt: a bulleted list with a second level, a numbered list with a
  nested level and prefix/suffix, a later list continuing it, a list
  restarted at 5, an item with two paragraphs, a list header, chapter
  numbering on headings.
* tables.odt: column widths, a horizontal and a vertical span, two header
  rows, cell borders, background and vertical alignment, a row height, a
  centred table, a table inside a cell.
* page.odt: US Letter landscape with custom margins, a header with a page
  number field, a footer with page count, a different first-page header;
  then an A4 portrait page style with its own footer, and a two-column
  section.
* notes.odt: a footnote and an endnote, a comment over a range and a
  comment at a point, a tracked insertion and deletion, bookmarks, title
  and page count fields, document properties.
* pictures.odt: an inline PNG, a PNG floating on the right with square
  wrap, a PNG anchored to the page, a text frame with formatted text, a
  drawing shape (left out), an SVG with no fallback (left out).
* text.fodt: text.odt's flat XML source, as it is (flat ODF opens too).

Regenerate with
`python3 tests/corpus/odt/make_fixtures.py tests/corpus/odt [soffice]`
where soffice defaults to ~/.cache/lo/lo.AppImage (LibreOffice 25.8).
LibreOffice output carries timestamps, so bytes differ between runs.
"""
import base64
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/odt").resolve()
soffice = Path(sys.argv[2] if len(sys.argv) > 2 else Path.home() / ".cache/lo/lo.AppImage")

NS = """xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
 xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
 xmlns:xlink="http://www.w3.org/1999/xlink"
 xmlns:dc="http://purl.org/dc/elements/1.1/"
 xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
 xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
 xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0"
 office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text\""""

FONTS = """<office:font-face-decls>
 <style:font-face style:name="Liberation Serif" svg:font-family="'Liberation Serif'" style:font-family-generic="roman"/>
 <style:font-face style:name="Liberation Sans" svg:font-family="'Liberation Sans'" style:font-family-generic="swiss"/>
 <style:font-face style:name="Courier New" svg:font-family="'Courier New'" style:font-family-generic="modern"/>
</office:font-face-decls>"""

# Named styles shared by every document.
STYLES = """<style:default-style style:family="paragraph">
  <style:paragraph-properties style:tab-stop-distance="1.25cm"/>
  <style:text-properties style:font-name="Liberation Serif" fo:font-size="12pt" fo:language="en" fo:country="US"/>
 </style:default-style>
 <style:style style:name="Standard" style:family="paragraph" style:class="text"/>
 <style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body">
  <style:paragraph-properties fo:margin-top="0.423cm" fo:margin-bottom="0.212cm" fo:keep-with-next="always"/>
  <style:text-properties style:font-name="Liberation Sans" fo:font-size="14pt"/>
 </style:style>
 <style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="1">
  <style:text-properties fo:font-size="130%" fo:font-weight="bold"/>
 </style:style>
 <style:style style:name="Heading_20_2" style:display-name="Heading 2" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="2">
  <style:text-properties fo:font-size="115%" fo:font-weight="bold"/>
 </style:style>
 <style:style style:name="Heading_20_3" style:display-name="Heading 3" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="3">
  <style:text-properties fo:font-size="101%" fo:font-weight="bold"/>
 </style:style>
 <style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard">
  <style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.247cm" fo:line-height="115%"/>
 </style:style>
 <style:style style:name="Note_20_box" style:display-name="Note box" style:family="paragraph" style:parent-style-name="Text_20_body">
  <style:paragraph-properties fo:margin-left="1cm" fo:background-color="#eeeeee" fo:border="0.5pt solid #808080" fo:padding="0.1cm"/>
  <style:text-properties fo:font-style="italic" fo:color="#333333"/>
 </style:style>
 <style:style style:name="Header" style:family="paragraph" style:parent-style-name="Standard"/>
 <style:style style:name="Footer" style:family="paragraph" style:parent-style-name="Standard"/>
 <style:style style:name="Footnote" style:family="paragraph" style:parent-style-name="Standard">
  <style:text-properties fo:font-size="10pt"/>
 </style:style>
 <style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard"/>
 <style:style style:name="Internet_20_link" style:display-name="Internet link" style:family="text">
  <style:text-properties fo:color="#000080" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color"/>
 </style:style>
 <style:style style:name="Strong_20_Emphasis" style:display-name="Strong Emphasis" style:family="text">
  <style:text-properties fo:font-weight="bold"/>
 </style:style>
 <style:style style:name="Code" style:family="text">
  <style:text-properties style:font-name="Courier New" fo:color="#006400"/>
 </style:style>"""


def page_layouts(extra=""):
    return f"""<style:page-layout style:name="pm1">
  <style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" style:print-orientation="portrait"
   fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2cm" fo:margin-right="2cm"/>
 </style:page-layout>{extra}"""


def doc(body, auto="", master=None, layouts=None, styles_extra="", meta=""):
    master = master or '<style:master-page style:name="Standard" style:page-layout-name="pm1"/>'
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<office:document {NS}>
<office:meta>{meta}</office:meta>
{FONTS}
<office:styles>{STYLES}{styles_extra}</office:styles>
<office:automatic-styles>{layouts or page_layouts()}{auto}</office:automatic-styles>
<office:master-styles>{master}</office:master-styles>
<office:body><office:text>{body}</office:text></office:body>
</office:document>
"""


def png(w, h, rgb):
    raw = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


def binary(data):
    return f"<office:binary-data>{base64.b64encode(data).decode()}</office:binary-data>"


TEXT = doc(
    auto="""<style:style style:name="P1" style:family="paragraph" style:parent-style-name="Standard">
  <style:paragraph-properties fo:text-align="center"/></style:style>
 <style:style style:name="P2" style:family="paragraph" style:parent-style-name="Standard">
  <style:paragraph-properties fo:text-align="end"/></style:style>
 <style:style style:name="P3" style:family="paragraph" style:parent-style-name="Standard">
  <style:paragraph-properties fo:text-align="justify" fo:margin-left="2cm" fo:margin-right="1cm" fo:text-indent="0.5cm"
   fo:margin-top="12pt" fo:margin-bottom="6pt" fo:line-height="150%"/></style:style>
 <style:style style:name="P4" style:family="paragraph" style:parent-style-name="Standard">
  <style:paragraph-properties fo:margin-left="1cm">
   <style:tab-stops>
    <style:tab-stop style:position="5cm"/>
    <style:tab-stop style:position="10cm" style:type="right" style:leader-style="dotted" style:leader-text="."/>
   </style:tab-stops>
  </style:paragraph-properties></style:style>
 <style:style style:name="P5" style:family="paragraph" style:parent-style-name="Standard">
  <style:paragraph-properties fo:break-before="page"/></style:style>
 <style:style style:name="P6" style:family="paragraph" style:parent-style-name="Standard">
  <style:text-properties fo:font-size="9pt" fo:color="#0000ff"/></style:style>
 <style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
 <style:style style:name="T2" style:family="text"><style:text-properties fo:font-style="italic"/></style:style>
 <style:style style:name="T3" style:family="text"><style:text-properties style:text-underline-style="solid" style:text-underline-width="auto"/></style:style>
 <style:style style:name="T4" style:family="text"><style:text-properties style:text-underline-style="solid" style:text-underline-type="double"/></style:style>
 <style:style style:name="T5" style:family="text"><style:text-properties style:text-line-through-style="solid"/></style:style>
 <style:style style:name="T6" style:family="text"><style:text-properties fo:font-size="18pt" fo:color="#c00000"/></style:style>
 <style:style style:name="T7" style:family="text"><style:text-properties style:font-name="Courier New"/></style:style>
 <style:style style:name="T8" style:family="text"><style:text-properties fo:background-color="#ffff00"/></style:style>
 <style:style style:name="T9" style:family="text"><style:text-properties style:text-position="super 58%"/></style:style>
 <style:style style:name="T10" style:family="text"><style:text-properties style:text-position="sub 58%"/></style:style>
 <style:style style:name="T11" style:family="text"><style:text-properties fo:font-variant="small-caps" fo:letter-spacing="0.1cm"/></style:style>
 <style:style style:name="T12" style:family="text"><style:text-properties fo:background-color="#ccffcc"/></style:style>""",
    body="""<text:h text:style-name="Heading_20_1" text:outline-level="1">Laporan Kantor</text:h>
<text:h text:style-name="Heading_20_2" text:outline-level="2">Ringkasan</text:h>
<text:h text:style-name="Heading_20_3" text:outline-level="3">Detail</text:h>
<text:p text:style-name="Standard">Plain, <text:span text:style-name="T1">bold</text:span>, <text:span text:style-name="T2">italic</text:span>, <text:span text:style-name="T3">underlined</text:span>, <text:span text:style-name="T4">double</text:span>, <text:span text:style-name="T5">struck</text:span>, <text:span text:style-name="T6">big red</text:span>, <text:span text:style-name="T7">Courier New</text:span>, <text:span text:style-name="T8">highlighted</text:span>, <text:span text:style-name="T12">shaded</text:span>, E=mc<text:span text:style-name="T9">2</text:span>, H<text:span text:style-name="T10">2</text:span>O, <text:span text:style-name="T11">small caps</text:span>, <text:span text:style-name="Code">code style</text:span>, <text:span text:style-name="Strong_20_Emphasis">strong <text:span text:style-name="T2">and italic</text:span></text:span>.</text:p>
<text:p text:style-name="P1">Centred paragraph.</text:p>
<text:p text:style-name="P2">Right aligned paragraph.</text:p>
<text:p text:style-name="P3">Justified and indented paragraph with space before and after and one and a half line spacing, long enough to wrap onto a second line on an A4 page.</text:p>
<text:p text:style-name="P4">Name:<text:tab/>Kantor<text:tab/>12<text:line-break/>Second   line with<text:s text:c="3"/>spaces.</text:p>
<text:p text:style-name="Text_20_body">Body text paragraph.</text:p>
<text:p text:style-name="Note_20_box">A note box: a custom style based on Text Body.</text:p>
<text:p text:style-name="P6">Small blue paragraph.</text:p>
<text:p text:style-name="Standard">Bahasa Indonesia: Selamat pagi, apa kabar? Rp 1.250.000,00</text:p>
<text:p text:style-name="Standard">CJK: 你好世界 こんにちは 안녕하세요</text:p>
<text:p text:style-name="Standard">Emoji: 😀 👍🏽 ✓</text:p>
<text:p text:style-name="Standard">Visit <text:a xlink:type="simple" xlink:href="https://kantor.paparanai.id/" text:style-name="Internet_20_link">the Kantor site</text:a> or go to <text:a xlink:type="simple" xlink:href="#target">the target</text:a>.</text:p>
<text:p text:style-name="P5"><text:bookmark text:name="target"/>After a page break.</text:p>""",
)

LISTS = doc(
    styles_extra="""<text:outline-style style:name="Outline">
  <text:outline-level-style text:level="1" style:num-format="1" style:num-suffix=".">
   <style:list-level-properties text:list-level-position-and-space-mode="label-alignment">
    <style:list-level-label-alignment text:label-followed-by="space" fo:text-indent="0cm" fo:margin-left="0cm"/>
   </style:list-level-properties></text:outline-level-style>
  <text:outline-level-style text:level="2" style:num-format="1" text:display-levels="2">
   <style:list-level-properties text:list-level-position-and-space-mode="label-alignment">
    <style:list-level-label-alignment text:label-followed-by="space" fo:text-indent="0cm" fo:margin-left="0cm"/>
   </style:list-level-properties></text:outline-level-style>
 </text:outline-style>""",
    auto="""<text:list-style style:name="L1">
  <text:list-level-style-bullet text:level="1" text:bullet-char="•">
   <style:list-level-properties text:list-level-position-and-space-mode="label-alignment">
    <style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="1.27cm" fo:text-indent="-0.635cm" fo:margin-left="1.27cm"/>
   </style:list-level-properties></text:list-level-style-bullet>
  <text:list-level-style-bullet text:level="2" text:bullet-char="◦">
   <style:list-level-properties text:list-level-position-and-space-mode="label-alignment">
    <style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="1.905cm" fo:text-indent="-0.635cm" fo:margin-left="1.905cm"/>
   </style:list-level-properties></text:list-level-style-bullet>
 </text:list-style>
 <text:list-style style:name="L2">
  <text:list-level-style-number text:level="1" style:num-format="1" style:num-suffix=".">
   <style:list-level-properties text:list-level-position-and-space-mode="label-alignment">
    <style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="1.27cm" fo:text-indent="-0.635cm" fo:margin-left="1.27cm"/>
   </style:list-level-properties></text:list-level-style-number>
  <text:list-level-style-number text:level="2" style:num-format="a" style:num-prefix="(" style:num-suffix=")">
   <style:list-level-properties text:list-level-position-and-space-mode="label-alignment">
    <style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="1.905cm" fo:text-indent="-0.635cm" fo:margin-left="1.905cm"/>
   </style:list-level-properties></text:list-level-style-number>
 </text:list-style>
 <text:list-style style:name="L3">
  <text:list-level-style-number text:level="1" style:num-format="I" style:num-suffix=")" text:start-value="1">
   <style:list-level-properties text:space-before="0.5cm" text:min-label-width="1cm"/>
  </text:list-level-style-number>
 </text:list-style>""",
    body="""<text:h text:style-name="Heading_20_1" text:outline-level="1">Chapter one</text:h>
<text:p text:style-name="Standard">Shopping:</text:p>
<text:list xml:id="list1" text:style-name="L1">
 <text:list-item><text:p text:style-name="Standard">Rice</text:p></text:list-item>
 <text:list-item><text:p text:style-name="Standard">Fruit</text:p>
  <text:list><text:list-item><text:p text:style-name="Standard">Mango</text:p></text:list-item>
   <text:list-item><text:p text:style-name="Standard">Papaya</text:p></text:list-item></text:list>
 </text:list-item>
 <text:list-item><text:p text:style-name="Standard">Coffee</text:p></text:list-item>
</text:list>
<text:h text:style-name="Heading_20_2" text:outline-level="2">Steps</text:h>
<text:list xml:id="list2" text:style-name="L2">
 <text:list-item><text:p text:style-name="Standard">First step</text:p></text:list-item>
 <text:list-item><text:p text:style-name="Standard">Second step</text:p>
  <text:list><text:list-item><text:p text:style-name="Standard">Sub step</text:p></text:list-item>
   <text:list-item><text:p text:style-name="Standard">Another sub step</text:p></text:list-item></text:list>
 </text:list-item>
 <text:list-item><text:p text:style-name="Standard">Third step</text:p><text:p text:style-name="Standard">More about the third step.</text:p></text:list-item>
</text:list>
<text:p text:style-name="Standard">An interruption.</text:p>
<text:list xml:id="list3" text:continue-list="list2" text:style-name="L2">
 <text:list-item><text:p text:style-name="Standard">Fourth step</text:p></text:list-item>
</text:list>
<text:list xml:id="list4" text:style-name="L2">
 <text:list-header><text:p text:style-name="Standard">A list header</text:p></text:list-header>
 <text:list-item text:start-value="5"><text:p text:style-name="Standard">Fifth, restarted</text:p></text:list-item>
 <text:list-item><text:p text:style-name="Standard">Sixth</text:p></text:list-item>
</text:list>
<text:list xml:id="list5" text:style-name="L3">
 <text:list-item><text:p text:style-name="Standard">Roman one</text:p></text:list-item>
 <text:list-item><text:p text:style-name="Standard">Roman two</text:p></text:list-item>
</text:list>
<text:h text:style-name="Heading_20_1" text:outline-level="1">Chapter two</text:h>""",
)

TABLES = doc(
    auto="""<style:style style:name="Table1" style:family="table">
  <style:table-properties style:width="15cm" table:align="center"/></style:style>
 <style:style style:name="Table1.A" style:family="table-column"><style:table-column-properties style:column-width="3cm"/></style:style>
 <style:style style:name="Table1.B" style:family="table-column"><style:table-column-properties style:column-width="5cm"/></style:style>
 <style:style style:name="Table1.C" style:family="table-column"><style:table-column-properties style:column-width="7cm"/></style:style>
 <style:style style:name="Table1.1" style:family="table-row"><style:table-row-properties style:min-row-height="1cm"/></style:style>
 <style:style style:name="Table1.H" style:family="table-cell">
  <style:table-cell-properties fo:background-color="#dddddd" fo:padding="0.1cm" fo:border="0.5pt solid #000000" style:vertical-align="middle"/></style:style>
 <style:style style:name="Table1.D" style:family="table-cell">
  <style:table-cell-properties fo:padding="0.1cm" fo:border-left="0.5pt solid #000000" fo:border-right="0.5pt solid #000000" fo:border-top="none" fo:border-bottom="1.5pt double #ff0000"/></style:style>
 <style:style style:name="Table2" style:family="table"><style:table-properties style:width="4cm" table:align="left"/></style:style>
 <style:style style:name="Table2.A" style:family="table-column"><style:table-column-properties style:column-width="2cm"/></style:style>
 <style:style style:name="P1" style:family="paragraph" style:parent-style-name="Table_20_Contents">
  <style:text-properties fo:font-weight="bold"/></style:style>""",
    body="""<text:p text:style-name="Standard">Before the table.</text:p>
<table:table table:name="Table1" table:style-name="Table1">
 <table:table-column table:style-name="Table1.A"/>
 <table:table-column table:style-name="Table1.B"/>
 <table:table-column table:style-name="Table1.C"/>
 <table:table-header-rows>
  <table:table-row table:style-name="Table1.1">
   <table:table-cell table:style-name="Table1.H"><text:p text:style-name="P1">Item</text:p></table:table-cell>
   <table:table-cell table:style-name="Table1.H" table:number-columns-spanned="2"><text:p text:style-name="P1">Details</text:p></table:table-cell>
   <table:covered-table-cell/>
  </table:table-row>
  <table:table-row>
   <table:table-cell table:style-name="Table1.H"><text:p text:style-name="P1">#</text:p></table:table-cell>
   <table:table-cell table:style-name="Table1.H"><text:p text:style-name="P1">Qty</text:p></table:table-cell>
   <table:table-cell table:style-name="Table1.H"><text:p text:style-name="P1">Note</text:p></table:table-cell>
  </table:table-row>
 </table:table-header-rows>
 <table:table-row>
  <table:table-cell table:style-name="Table1.D" table:number-rows-spanned="2"><text:p text:style-name="Table_20_Contents">Merged down</text:p></table:table-cell>
  <table:table-cell table:style-name="Table1.D"><text:p text:style-name="Table_20_Contents">10</text:p></table:table-cell>
  <table:table-cell table:style-name="Table1.D"><text:p text:style-name="Table_20_Contents">First</text:p></table:table-cell>
 </table:table-row>
 <table:table-row>
  <table:covered-table-cell/>
  <table:table-cell table:style-name="Table1.D"><text:p text:style-name="Table_20_Contents">20</text:p></table:table-cell>
  <table:table-cell table:style-name="Table1.D">
   <text:p text:style-name="Table_20_Contents">Inner table:</text:p>
   <table:table table:name="Table2" table:style-name="Table2">
    <table:table-column table:style-name="Table2.A" table:number-columns-repeated="2"/>
    <table:table-row>
     <table:table-cell><text:p text:style-name="Table_20_Contents">a</text:p></table:table-cell>
     <table:table-cell><text:p text:style-name="Table_20_Contents">b</text:p></table:table-cell>
    </table:table-row>
   </table:table>
   <text:p text:style-name="Table_20_Contents"/>
  </table:table-cell>
 </table:table-row>
</table:table>
<text:p text:style-name="Standard">After the table.</text:p>""",
)

PAGE = doc(
    layouts="""<style:page-layout style:name="pmLetter">
  <style:page-layout-properties fo:page-width="27.94cm" fo:page-height="21.59cm" style:print-orientation="landscape"
   fo:margin-top="1.5cm" fo:margin-bottom="1.5cm" fo:margin-left="2.5cm" fo:margin-right="2cm"/>
  <style:header-style><style:header-footer-properties fo:min-height="0.5cm" fo:margin-bottom="0.5cm"/></style:header-style>
  <style:footer-style><style:header-footer-properties fo:min-height="0.5cm" fo:margin-top="0.5cm"/></style:footer-style>
 </style:page-layout>
 <style:page-layout style:name="pmA4">
  <style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" style:print-orientation="portrait"
   fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2cm" fo:margin-right="2cm"/>
  <style:footer-style><style:header-footer-properties fo:min-height="0cm" fo:margin-top="0.3cm"/></style:footer-style>
 </style:page-layout>""",
    auto="""<style:style style:name="P1" style:family="paragraph" style:parent-style-name="Standard" style:master-page-name="Portrait">
  <style:paragraph-properties style:page-number="auto"/></style:style>
 <style:style style:name="Sect1" style:family="section">
  <style:section-properties><style:columns fo:column-count="2" fo:column-gap="1cm"><style:column-sep style:width="0.02cm" style:color="#000000" style:height="100%"/></style:columns></style:section-properties>
 </style:style>""",
    master="""<style:master-page style:name="Standard" style:page-layout-name="pmLetter">
  <style:header><text:p text:style-name="Header">Kantor report, page <text:page-number text:select-page="current">1</text:page-number></text:p></style:header>
  <style:header-first><text:p text:style-name="Header">Cover page header</text:p></style:header-first>
  <style:footer><text:p text:style-name="Footer">Pages: <text:page-count>3</text:page-count></text:p></style:footer>
 </style:master-page>
 <style:master-page style:name="Portrait" style:page-layout-name="pmA4">
  <style:footer><text:p text:style-name="Footer">Portrait footer</text:p></style:footer>
 </style:master-page>""",
    body="""<text:p text:style-name="Standard">A landscape Letter page.</text:p>
<text:p text:style-name="Standard">Second paragraph.</text:p>
<text:p text:style-name="P1">An A4 portrait page.</text:p>
<text:section text:style-name="Sect1" text:name="Columns">
 <text:p text:style-name="Standard">Text in two columns. Text in two columns. Text in two columns.</text:p>
 <text:p text:style-name="Standard">More text in two columns.</text:p>
</text:section>
<text:p text:style-name="Standard">One column again.</text:p>""",
)

NOTES = doc(
    meta="""<dc:title>Catatan rapat</dc:title><dc:subject>Notes fixture</dc:subject>
 <meta:initial-creator>Kantor corpus</meta:initial-creator><dc:creator>Kantor corpus</dc:creator>
 <meta:creation-date>2026-11-13T00:00:00</meta:creation-date><dc:date>2026-11-13T00:00:00</dc:date>
 <meta:keyword>kantor</meta:keyword><meta:keyword>odt</meta:keyword>""",
    body="""<text:tracked-changes text:track-changes="false">
 <text:changed-region xml:id="ct1" text:id="ct1"><text:insertion><office:change-info><dc:creator>Budi</dc:creator><dc:date>2026-11-13T10:00:00</dc:date></office:change-info></text:insertion></text:changed-region>
 <text:changed-region xml:id="ct2" text:id="ct2"><text:deletion><office:change-info><dc:creator>Sari</dc:creator><dc:date>2026-11-13T11:00:00</dc:date></office:change-info><text:p text:style-name="Standard">old words </text:p></text:deletion></text:changed-region>
</text:tracked-changes>
<text:p text:style-name="Standard">Title: <text:title>Catatan rapat</text:title>, <text:page-count>1</text:page-count> page.</text:p>
<text:p text:style-name="Standard">A footnote here<text:note text:id="ftn1" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p text:style-name="Footnote">The footnote text.</text:p></text:note-body></text:note> and an endnote there<text:note text:id="ftn2" text:note-class="endnote"><text:note-citation>i</text:note-citation><text:note-body><text:p text:style-name="Standard">The endnote text.</text:p></text:note-body></text:note>.</text:p>
<text:p text:style-name="Standard">Commented <office:annotation office:name="c1"><dc:creator>Budi Santoso</dc:creator><dc:date>2026-11-13T09:00:00</dc:date><text:p>Check this span.</text:p></office:annotation>span of text<office:annotation-end office:name="c1"/> and a point<office:annotation><dc:creator>Sari</dc:creator><dc:date>2026-11-13T09:30:00</dc:date><text:p>A point comment.</text:p><text:p>Second line.</text:p></office:annotation>.</text:p>
<text:p text:style-name="Standard">Tracked: <text:change-start text:change-id="ct1"/>inserted words<text:change-end text:change-id="ct1"/> and <text:change text:change-id="ct2"/>kept.</text:p>
<text:p text:style-name="Standard"><text:bookmark-start text:name="span"/>Bookmarked text<text:bookmark-end text:name="span"/>.</text:p>""",
)

PIX_RED = png(40, 20, (200, 30, 30))
PIX_BLUE = png(30, 30, (30, 60, 200))
SVG = b'<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="green"/></svg>'

PICTURES = doc(
    styles_extra="""<style:style style:name="Frame" style:family="graphic">
  <style:graphic-properties svg:x="0cm" svg:y="0cm" fo:margin-left="0.2cm" fo:margin-right="0.2cm" fo:margin-top="0.2cm" fo:margin-bottom="0.2cm"
   style:wrap="parallel" style:vertical-pos="top" style:vertical-rel="paragraph-content" style:horizontal-pos="center" style:horizontal-rel="paragraph-content"
   fo:padding="0.15cm" fo:border="0.06pt solid #000000"/></style:style>""",
    auto="""<style:style style:name="fr1" style:family="graphic">
  <style:graphic-properties style:wrap="none" style:vertical-pos="top" style:vertical-rel="baseline"/></style:style>
 <style:style style:name="fr2" style:family="graphic">
  <style:graphic-properties style:wrap="parallel" style:horizontal-pos="right" style:horizontal-rel="paragraph" style:vertical-pos="from-top" style:vertical-rel="paragraph" fo:margin-left="0.2cm" fo:margin-right="0.2cm"/></style:style>
 <style:style style:name="fr3" style:family="graphic">
  <style:graphic-properties style:wrap="run-through" style:run-through="foreground" style:horizontal-pos="from-left" style:horizontal-rel="page" style:vertical-pos="from-top" style:vertical-rel="page"/></style:style>
 <style:style style:name="fr4" style:family="graphic" style:parent-style-name="Frame">
  <style:graphic-properties style:wrap="parallel" style:horizontal-pos="from-left" style:horizontal-rel="paragraph" style:vertical-pos="from-top" style:vertical-rel="paragraph"
   draw:fill="solid" draw:fill-color="#ffffcc" fo:background-color="#ffffcc" fo:border="0.5pt solid #000000"/></style:style>
 <style:style style:name="gr1" style:family="graphic"><style:graphic-properties draw:fill-color="#729fcf"/></style:style>
 <style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>""",
    body=f"""<draw:frame draw:style-name="fr3" draw:name="PagePicture" text:anchor-type="page" text:anchor-page-number="1" svg:x="15cm" svg:y="1cm" svg:width="2cm" svg:height="2cm"><draw:image>{binary(PIX_BLUE)}</draw:image></draw:frame>
<text:p text:style-name="Standard">An inline picture <draw:frame draw:style-name="fr1" draw:name="Inline" text:anchor-type="as-char" svg:width="2cm" svg:height="1cm"><draw:image>{binary(PIX_RED)}</draw:image></draw:frame> in the text.</text:p>
<text:p text:style-name="Standard"><draw:frame draw:style-name="fr2" draw:name="Floating" text:anchor-type="paragraph" svg:y="0.2cm" svg:width="3cm" svg:height="3cm"><draw:image>{binary(PIX_BLUE)}</draw:image></draw:frame>A paragraph with a floating picture on the right. Text wraps around it on the left side of the page.</text:p>
<text:p text:style-name="Standard"><draw:frame draw:style-name="fr4" draw:name="TextFrame" text:anchor-type="paragraph" svg:x="1cm" svg:y="0cm" svg:width="6cm" svg:height="2cm"><draw:text-box><text:p text:style-name="Standard">Text in a <text:span text:style-name="T1">frame</text:span>.</text:p><text:p text:style-name="Standard">Second frame line.</text:p></draw:text-box></draw:frame>A paragraph with a text frame.</text:p>
<text:p text:style-name="Standard"><draw:custom-shape draw:style-name="gr1" draw:name="Shape" text:anchor-type="paragraph" svg:width="2cm" svg:height="1cm"><draw:enhanced-geometry draw:type="rectangle"/></draw:custom-shape>A paragraph with a drawing shape.</text:p>
<text:p text:style-name="Standard">An SVG without fallback: <draw:frame draw:style-name="fr1" draw:name="Svg" text:anchor-type="as-char" svg:width="1cm" svg:height="1cm"><draw:image>{binary(SVG)}</draw:image></draw:frame>.</text:p>""",
)

DOCS = {
    "text": TEXT,
    "lists": LISTS,
    "tables": TABLES,
    "page": PAGE,
    "notes": NOTES,
    "pictures": PICTURES,
}


def main():
    out.mkdir(parents=True, exist_ok=True)
    base = Path(os.environ.get("TMPDIR", Path.home() / ".cache/ktmp"))
    base.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=base) as t:
        tmp = Path(t)
        for name, xml in DOCS.items():
            (tmp / f"{name}.fodt").write_text(xml, encoding="utf-8")
        profile = tmp / "profile"
        cmd = [
            str(soffice),
            f"-env:UserInstallation=file://{profile}",
            "--headless",
            "--convert-to",
            "odt:writer8",
            "--outdir",
            str(tmp / "odt"),
            *[str(tmp / f"{n}.fodt") for n in DOCS],
        ]
        # 81: LibreOffice made a new profile and wants a restart; run again.
        if subprocess.run(cmd).returncode == 81:
            subprocess.run(cmd, check=True)
        for n in DOCS:
            shutil.copy(tmp / "odt" / f"{n}.odt", out / f"{n}.odt")
            print(out / f"{n}.odt")
    (out / "text.fodt").write_text(TEXT, encoding="utf-8")
    print(out / "text.fodt")


main()
