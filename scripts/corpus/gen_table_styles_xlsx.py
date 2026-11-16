#!/usr/bin/env python3
"""Generate tests/corpus/xlsx/24_table_styles.xlsx: custom table styles
(`<tableStyles>` in styles.xml), as Excel writes them after Table Design >
New Table Style.

Raw SpreadsheetML written here (standard library only): neither openpyxl
nor XlsxWriter writes custom table styles. Sheet "Sales" (A1:C7) holds:

* table "Sales" (A1:C7, header and total rows, banded rows, first and
  last column)
  styled "Brand Purple": header row purple fill, white bold text; whole
  table thin purple outline and inside lines; first row stripe 2 rows of
  light purple; first column italic; total row double line above, bold;
  last total cell red text (shown with Last Column, as in Excel).
* a conditional format on C2:C6 (> 100, red fill) whose dxf is dxf 0: the
  table style's dxfs come after it, as in Excel files with both.

styles.xml also has "Pivot Only" (table="0", a pivot table style) and
defaultTableStyle="Brand Purple". Sheet "Other" has a table "Plain" in
TableStyleLight9. CC0-1.0 like the rest of the corpus. Deterministic
(fixed zip timestamps): `--check` compares parts.

    python3 scripts/corpus/gen_table_styles_xlsx.py           # write
    python3 scripts/corpus/gen_table_styles_xlsx.py --check   # CI
"""

from __future__ import annotations

import os
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "tests", "corpus", "xlsx", "24_table_styles.xlsx")

NS_MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS_PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
CT = "application/vnd.openxmlformats-officedocument.spreadsheetml."
HEAD = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'

PURPLE = "FF7030A0"
LIGHT = "FFE4DFEC"

DXFS = [
    # 0: the conditional format's red fill.
    '<dxf><font><color rgb="FF9C0006"/></font><fill><patternFill><bgColor rgb="FFFFC7CE"/></patternFill></fill></dxf>',
    # 1: whole table.
    f'<dxf><border><left style="thin"><color rgb="{PURPLE}"/></left><right style="thin"><color rgb="{PURPLE}"/></right>'
    f'<top style="thin"><color rgb="{PURPLE}"/></top><bottom style="thin"><color rgb="{PURPLE}"/></bottom>'
    f'<vertical style="thin"><color rgb="{PURPLE}"/></vertical><horizontal style="thin"><color rgb="{PURPLE}"/></horizontal></border></dxf>',
    # 2: header row.
    f'<dxf><font><b/><color rgb="FFFFFFFF"/></font><fill><patternFill patternType="solid"><fgColor rgb="{PURPLE}"/><bgColor rgb="{PURPLE}"/></patternFill></fill></dxf>',
    # 3: first row stripe.
    f'<dxf><fill><patternFill patternType="solid"><fgColor rgb="{LIGHT}"/><bgColor rgb="{LIGHT}"/></patternFill></fill></dxf>',
    # 4: first column.
    "<dxf><font><i/></font></dxf>",
    # 5: total row.
    f'<dxf><font><b/></font><border><top style="double"><color rgb="{PURPLE}"/></top></border></dxf>',
    # 6: last total cell.
    '<dxf><font><color rgb="FFFF0000"/></font></dxf>',
    # 7: pivot style's header.
    '<dxf><font><b/></font><fill><patternFill patternType="solid"><fgColor rgb="FFFFC000"/><bgColor rgb="FFFFC000"/></patternFill></fill></dxf>',
]

TABLE_STYLES = (
    '<tableStyles count="2" defaultTableStyle="Brand Purple" defaultPivotStyle="PivotStyleLight16">'
    '<tableStyle name="Brand Purple" pivot="0" count="6">'
    '<tableStyleElement type="wholeTable" dxfId="1"/>'
    '<tableStyleElement type="headerRow" dxfId="2"/>'
    '<tableStyleElement type="totalRow" dxfId="5"/>'
    '<tableStyleElement type="firstColumn" dxfId="4"/>'
    '<tableStyleElement type="firstRowStripe" size="2" dxfId="3"/>'
    '<tableStyleElement type="lastTotalCell" dxfId="6"/>'
    "</tableStyle>"
    '<tableStyle name="Pivot Only" table="0" count="1"><tableStyleElement type="headerRow" dxfId="7"/></tableStyle>'
    "</tableStyles>"
)


def styles() -> str:
    return (
        HEAD + f'<styleSheet xmlns="{NS_MAIN}"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
        '<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>'
        '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
        '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        '<cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>'
        '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>'
        f'<dxfs count="{len(DXFS)}">' + "".join(DXFS) + "</dxfs>" + TABLE_STYLES + "</styleSheet>"
    )


def cell(r: int, c: int, v) -> str:
    ref = f"{chr(65 + c)}{r + 1}"
    if isinstance(v, (int, float)):
        return f'<c r="{ref}"><v>{v}</v></c>'
    if isinstance(v, tuple):
        return f'<c r="{ref}"><f>{v[0][1:]}</f><v>{v[1]}</v></c>'
    return f'<c r="{ref}" t="inlineStr"><is><t>{v}</t></is></c>'


def sheet(rows: list[list], extra: str = "", table_rid: str | None = None) -> str:
    data = "".join(
        f'<row r="{r + 1}">' + "".join(cell(r, c, v) for c, v in enumerate(row) if v is not None) + "</row>"
        for r, row in enumerate(rows)
    )
    parts = f'<tableParts count="1"><tablePart r:id="{table_rid}"/></tableParts>' if table_rid else ""
    return HEAD + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData>{data}</sheetData>{extra}{parts}</worksheet>'


def table(id_: int, name: str, ref: str, cols: list[str], style: str, totals: bool, edge_cols: bool, filter_ref: str) -> str:
    tcols = ""
    for i, c in enumerate(cols):
        attr = ""
        if totals and i == 0:
            attr = ' totalsRowLabel="Total"'
        elif totals and i == len(cols) - 1:
            attr = ' totalsRowFunction="sum"'
        tcols += f'<tableColumn id="{i + 1}" name="{c}"{attr}/>'
    t = ' totalsRowCount="1"' if totals else ""
    return (
        HEAD + f'<table xmlns="{NS_MAIN}" id="{id_}" name="{name}" displayName="{name}" ref="{ref}"{t}>'
        f'<autoFilter ref="{filter_ref}"/><tableColumns count="{len(cols)}">{tcols}</tableColumns>'
        f'<tableStyleInfo name="{style}" showFirstColumn="{int(edge_cols)}" showLastColumn="{int(edge_cols)}" showRowStripes="1" showColumnStripes="0"/></table>'
    )


def parts() -> dict[str, str]:
    sales = [["Region", "Units", "Amount"], ["North", 10, 120], ["South", 4, 95], ["East", 7, 143],
             ["West", 3, 60], ["Central", 12, 210], ["Total", None, ("=SUBTOTAL(109,Sales[Amount])", 628)]]
    cf = ('<conditionalFormatting sqref="C2:C6"><cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan">'
          "<formula>100</formula></cfRule></conditionalFormatting>")
    return {
        "[Content_Types].xml": HEAD
        + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        f'<Override PartName="/xl/workbook.xml" ContentType="{CT}sheet.main+xml"/>'
        f'<Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT}worksheet+xml"/>'
        f'<Override PartName="/xl/worksheets/sheet2.xml" ContentType="{CT}worksheet+xml"/>'
        f'<Override PartName="/xl/styles.xml" ContentType="{CT}styles+xml"/>'
        f'<Override PartName="/xl/tables/table1.xml" ContentType="{CT}table+xml"/>'
        f'<Override PartName="/xl/tables/table2.xml" ContentType="{CT}table+xml"/>'
        "</Types>",
        "_rels/.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}officeDocument" Target="xl/workbook.xml"/></Relationships>',
        "xl/workbook.xml": HEAD
        + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets>'
        '<sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId2"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}">'
        f'<Relationship Id="rId1" Type="{REL}worksheet" Target="worksheets/sheet1.xml"/>'
        f'<Relationship Id="rId2" Type="{REL}worksheet" Target="worksheets/sheet2.xml"/>'
        f'<Relationship Id="rId3" Type="{REL}styles" Target="styles.xml"/></Relationships>',
        "xl/styles.xml": styles(),
        "xl/worksheets/sheet1.xml": sheet(sales, cf, "rId1"),
        "xl/worksheets/_rels/sheet1.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}table" Target="../tables/table1.xml"/></Relationships>',
        "xl/worksheets/sheet2.xml": sheet([["Name", "Score"], ["Ann", 3], ["Bo", 5]], "", "rId1"),
        "xl/worksheets/_rels/sheet2.xml.rels": HEAD
        + f'<Relationships xmlns="{NS_PKG}"><Relationship Id="rId1" Type="{REL}table" Target="../tables/table2.xml"/></Relationships>',
        "xl/tables/table1.xml": table(1, "Sales", "A1:C7", ["Region", "Units", "Amount"], "Brand Purple", True, True, "A1:C6"),
        "xl/tables/table2.xml": table(2, "Plain", "A1:B3", ["Name", "Score"], "TableStyleLight9", False, False, "A1:B3"),
    }


def write(path: str) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts().items():
            info = zipfile.ZipInfo(name, (2026, 11, 16, 9, 0, 0))
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
