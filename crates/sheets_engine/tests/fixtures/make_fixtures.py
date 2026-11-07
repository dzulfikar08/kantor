#!/usr/bin/env python3
"""Regenerate the tiny import fixtures used by sheets_engine's tests.

    python3 -m venv /tmp/fx && /tmp/fx/bin/pip install xlwt
    /tmp/fx/bin/python make_fixtures.py

`simple.ods` needs only the standard library; `simple.xls` needs xlwt.
Both hold the same content:

  Data!A1:C4   Item | Qty | When
               Apples | 3 | 2024-01-15 (date)
               Pears  | 4 | (empty)
               =text  | =SUM(B2:B3) | =B4*2
  Other!A1     =Data!B4+1
"""

import datetime
import os
import re
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))


ODS_HEAD = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2"
  office:version="1.2">
<office:body><office:spreadsheet>
"""
ODS_TAIL = """</office:spreadsheet></office:body>
</office:document-content>
"""


def write_ods(path):
    body = """<table:table table:name="Data">
<table:table-row>
  <table:table-cell office:value-type="string"><text:p>Item</text:p></table:table-cell>
  <table:table-cell office:value-type="string"><text:p>Qty</text:p></table:table-cell>
  <table:table-cell office:value-type="string"><text:p>When</text:p></table:table-cell>
</table:table-row>
<table:table-row>
  <table:table-cell office:value-type="string"><text:p>Apples</text:p></table:table-cell>
  <table:table-cell office:value-type="float" office:value="3"><text:p>3</text:p></table:table-cell>
  <table:table-cell office:value-type="date" office:date-value="2024-01-15"><text:p>2024-01-15</text:p></table:table-cell>
</table:table-row>
<table:table-row>
  <table:table-cell office:value-type="string"><text:p>Pears</text:p></table:table-cell>
  <table:table-cell office:value-type="float" office:value="4"><text:p>4</text:p></table:table-cell>
</table:table-row>
<table:table-row>
  <table:table-cell office:value-type="string"><text:p>=text</text:p></table:table-cell>
  <table:table-cell table:formula="of:=SUM([.B2:.B3])" office:value-type="float" office:value="7"><text:p>7</text:p></table:table-cell>
  <table:table-cell table:formula="of:=[.B4]*2" office:value-type="float" office:value="14"><text:p>14</text:p></table:table-cell>
</table:table-row>
</table:table>
<table:table table:name="Other">
<table:table-row>
  <table:table-cell table:formula="of:=[$Data.B4]+1" office:value-type="float" office:value="8"><text:p>8</text:p></table:table-cell>
</table:table-row>
</table:table>
"""
    write_ods_file(path, ODS_HEAD + body + ODS_TAIL)


def write_ods_file(path, content):
    manifest = """<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
 <manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>
"""
    # calamine rejects whitespace between cells (LibreOffice never writes any).
    content = re.sub(r">\s+<", "><", content)
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("mimetype", "application/vnd.oasis.opendocument.spreadsheet",
                   compress_type=zipfile.ZIP_STORED)
        z.writestr("content.xml", content, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("META-INF/manifest.xml", manifest, compress_type=zipfile.ZIP_DEFLATED)


def write_fallback_ods(path):
    """Formulas Kantor can't reproduce: an unknown function, and a cached
    result that disagrees with the formula. Both must keep the cached value."""
    body = """<table:table table:name="Sheet1">
<table:table-row>
  <table:table-cell office:value-type="float" office:value="2"><text:p>2</text:p></table:table-cell>
  <table:table-cell table:formula="of:=ORG.EXAMPLE.NOSUCHFUNCTION([.A1])" office:value-type="float" office:value="42"><text:p>42</text:p></table:table-cell>
  <table:table-cell table:formula="of:=[.A1]*100" office:value-type="float" office:value="999"><text:p>999</text:p></table:table-cell>
  <table:table-cell table:formula="of:=[.A1]+1" office:value-type="float" office:value="3"><text:p>3</text:p></table:table-cell>
</table:table-row>
</table:table>
"""
    write_ods_file(path, ODS_HEAD + body + ODS_TAIL)


def write_xls(path):
    import xlwt

    wb = xlwt.Workbook()
    data = wb.add_sheet("Data")
    other = wb.add_sheet("Other")
    date_style = xlwt.easyxf(num_format_str="YYYY-MM-DD")
    for col, title in enumerate(["Item", "Qty", "When"]):
        data.write(0, col, title)
    data.write(1, 0, "Apples")
    data.write(1, 1, 3)
    data.write(1, 2, datetime.date(2024, 1, 15), date_style)
    data.write(2, 0, "Pears")
    data.write(2, 1, 4)
    data.write(3, 0, "=text")
    data.write(3, 1, xlwt.Formula("SUM(B2:B3)"))
    data.write(3, 2, xlwt.Formula("B4*2"))
    other.write(0, 0, xlwt.Formula("Data!B4+1"))
    wb.save(path)


if __name__ == "__main__":
    write_ods(os.path.join(HERE, "simple.ods"))
    write_fallback_ods(os.path.join(HERE, "fallback.ods"))
    write_xls(os.path.join(HERE, "simple.xls"))
