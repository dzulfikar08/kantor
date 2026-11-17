#!/usr/bin/env python3
"""Generate tests/corpus/xls/odsdraw.ods (CC0, made-up content): the drawing
objects of an OpenDocument spreadsheet that LibreOffice's own conversions of
the other fixtures don't have, written as ODF XML directly (no LibreOffice
needed, so the file is the same on every run).

    python3 scripts/corpus/gen_ods_drawings.py [--check]

Sheet "Shapes", objects anchored to the sheet:
* freeform shapes: a draw:path with lines and a cubic curve ("Wave"), a
  draw:polygon ("Arrow polygon"), a draw:polyline ("Zigzag"), a
  draw:regular-polygon with 6 corners ("Hexagon") and a 5-pointed star
  ("Star");
* a group (draw:g) of a rectangle and an ellipse ("Pair") with a group
  inside it holding a line ("Inner");
* a callout (draw:caption, "Callout") with text;
* a rectangle inside a hyperlink (draw:a, "Linked box");
* form controls (draw:control with office:forms): a button "Run", a
  checked check box "Agree", a list box showing "Beta", a text field
  "typed" and a group box "Options";
* an embedded object that isn't a chart, with its replacement picture
  (a PNG in ObjectReplacements/), "Embedded".
"""

import struct
import sys
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests" / "corpus" / "xls" / "odsdraw.ods"

NS = (
    'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
    'xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" '
    'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
    'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
    'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
    'xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" '
    'xmlns:xlink="http://www.w3.org/1999/xlink" '
    'xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" '
    'xmlns:form="urn:oasis:names:tc:opendocument:xmlns:form:1.0" '
    'xmlns:xforms="http://www.w3.org/2002/xforms" '
    'office:version="1.3"'
)


def png(w: int, h: int, rgb: tuple[int, int, int]) -> bytes:
    def chunk(t: bytes, d: bytes) -> bytes:
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))

    raw = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def content() -> str:
    styles = """
<office:automatic-styles>
 <style:style style:name="blue" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#4472c4" draw:stroke="solid" svg:stroke-color="#203864" svg:stroke-width="0.03cm"/></style:style>
 <style:style style:name="orange" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#ed7d31" draw:stroke="solid" svg:stroke-color="#843c0c"/></style:style>
 <style:style style:name="green" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#70ad47" draw:stroke="none"/></style:style>
 <style:style style:name="line" style:family="graphic"><style:graphic-properties draw:fill="none" draw:stroke="solid" svg:stroke-color="#c00000" svg:stroke-width="0.05cm"/></style:style>
 <style:style style:name="note" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#fff2cc" draw:stroke="solid" svg:stroke-color="#7f6000" draw:textarea-vertical-align="middle"/></style:style>
</office:automatic-styles>"""
    forms = """
<office:forms form:automatic-focus="false" form:apply-design-mode="false">
 <form:form form:name="Form" form:apply-filter="true" form:command-type="table">
  <form:button xml:id="ctrl1" form:id="ctrl1" form:name="Run" form:label="Run" form:button-type="push"/>
  <form:checkbox xml:id="ctrl2" form:id="ctrl2" form:name="Agree" form:label="Agree" form:current-state="checked"/>
  <form:listbox xml:id="ctrl3" form:id="ctrl3" form:name="Pick" form:dropdown="true">
   <form:option form:label="Alpha"/><form:option form:label="Beta" form:current-selected="true"/>
  </form:listbox>
  <form:text xml:id="ctrl4" form:id="ctrl4" form:name="Field" form:current-value="typed"/>
  <form:frame xml:id="ctrl5" form:id="ctrl5" form:name="Box" form:label="Options"/>
 </form:form>
</office:forms>"""
    shapes = """
<table:shapes>
 <draw:path draw:name="Wave" draw:style-name="blue" svg:x="0.5cm" svg:y="0.5cm" svg:width="4cm" svg:height="2cm" svg:viewBox="0 0 4000 2000" svg:d="M 0 2000 L 0 1000 C 1000 0 3000 2000 4000 1000 L 4000 2000 Z"/>
 <draw:polygon draw:name="Arrow polygon" draw:style-name="orange" svg:x="5cm" svg:y="0.5cm" svg:width="3cm" svg:height="2cm" svg:viewBox="0 0 3000 2000" draw:points="0,500 2000,500 2000,0 3000,1000 2000,2000 2000,1500 0,1500"/>
 <draw:polyline draw:name="Zigzag" draw:style-name="line" svg:x="9cm" svg:y="0.5cm" svg:width="4cm" svg:height="2cm" svg:viewBox="0 0 4000 2000" draw:points="0,2000 1000,0 2000,2000 3000,0 4000,2000"/>
 <draw:regular-polygon draw:name="Hexagon" draw:style-name="green" svg:x="0.5cm" svg:y="3cm" svg:width="2.5cm" svg:height="2.5cm" draw:corners="6"/>
 <draw:regular-polygon draw:name="Star" draw:style-name="orange" svg:x="3.5cm" svg:y="3cm" svg:width="2.5cm" svg:height="2.5cm" draw:corners="5" draw:concave="true" draw:sharpness="60%"/>
 <draw:g draw:name="Pair">
  <draw:rect draw:name="Pair square" draw:style-name="blue" svg:x="7cm" svg:y="3cm" svg:width="2cm" svg:height="2cm"/>
  <draw:ellipse draw:name="Pair circle" draw:style-name="green" svg:x="9.5cm" svg:y="3cm" svg:width="2cm" svg:height="2cm"/>
  <draw:g draw:name="Inner">
   <draw:line draw:name="Pair line" draw:style-name="line" svg:x1="7cm" svg:y1="5.5cm" svg:x2="11.5cm" svg:y2="5.5cm"/>
  </draw:g>
 </draw:g>
 <draw:caption draw:name="Callout" draw:style-name="note" svg:x="0.5cm" svg:y="6.5cm" svg:width="4cm" svg:height="1.5cm" draw:caption-point-x="-1cm" draw:caption-point-y="2cm"><text:p>Look here</text:p></draw:caption>
 <draw:a xlink:type="simple" xlink:href="https://example.com/"><draw:rect draw:name="Linked box" draw:style-name="blue" svg:x="5cm" svg:y="6.5cm" svg:width="3cm" svg:height="1.5cm"><text:p>Link</text:p></draw:rect></draw:a>
 <draw:control draw:name="Run button" draw:control="ctrl1" svg:x="0.5cm" svg:y="9cm" svg:width="2.5cm" svg:height="0.8cm"/>
 <draw:control draw:name="Agree box" draw:control="ctrl2" svg:x="3.5cm" svg:y="9cm" svg:width="2.5cm" svg:height="0.8cm"/>
 <draw:control draw:name="Pick list" draw:control="ctrl3" svg:x="6.5cm" svg:y="9cm" svg:width="3cm" svg:height="0.8cm"/>
 <draw:control draw:name="Field box" draw:control="ctrl4" svg:x="10cm" svg:y="9cm" svg:width="3cm" svg:height="0.8cm"/>
 <draw:control draw:name="Options frame" draw:control="ctrl5" svg:x="0.5cm" svg:y="10.5cm" svg:width="6cm" svg:height="2cm"/>
 <draw:frame draw:name="Embedded" svg:x="9cm" svg:y="6.5cm" svg:width="3cm" svg:height="2cm"><draw:object xlink:href="./Object 1" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/><draw:image xlink:href="./ObjectReplacements/Object 1" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/></draw:frame>
</table:shapes>"""
    return (f'<?xml version="1.0" encoding="UTF-8"?>\n<office:document-content {NS}>{styles}'
            f'<office:body><office:spreadsheet><table:table table:name="Shapes">{forms}{shapes}'
            '<table:table-column table:number-columns-repeated="8"/>'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>Shapes</text:p>'
            '</table:table-cell></table:table-row></table:table></office:spreadsheet></office:body>'
            '</office:document-content>')


def build() -> bytes:
    import io

    manifest = (
        '<?xml version="1.0" encoding="UTF-8"?>\n<manifest:manifest '
        'xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">'
        '<manifest:file-entry manifest:full-path="/" '
        'manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>'
        '<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>'
        '<manifest:file-entry manifest:full-path="ObjectReplacements/Object 1" '
        'manifest:media-type="image/png"/>'
        '</manifest:manifest>')
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        def add(name, data, method=zipfile.ZIP_DEFLATED):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = method
            z.writestr(info, data)

        add("mimetype", "application/vnd.oasis.opendocument.spreadsheet", zipfile.ZIP_STORED)
        add("content.xml", content())
        add("ObjectReplacements/Object 1", png(30, 20, (0x5B, 0x9B, 0xD5)))
        add("META-INF/manifest.xml", manifest)
    return buf.getvalue()


def main() -> None:
    data = build()
    if "--check" in sys.argv:
        if OUT.read_bytes() != data:
            sys.exit(f"{OUT} is out of date: run {sys.argv[0]}")
        return
    OUT.write_bytes(data)
    print(OUT)


if __name__ == "__main__":
    main()
