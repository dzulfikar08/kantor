#!/usr/bin/env python3
"""Write tests/corpus/xlsx/25_slicers.xlsx: slicers and a timeline as Excel
2010-2013 write them, over the data of gen_pivot_xlsx.py.

* `Data`: the `Sales` table (A1:E13) and a **table slicer** on its Product
  column (`G2`), Excel 2013's `x15:tableSlicerCache`;
* `Report`: `PivotTable1` (rows Product, Sum of Revenue) with a **pivot
  slicer** on Region (`E2`, a field on no axis of the pivot table, all items
  selected) and a **timeline** on Date (`E12`, months, no selection).

Every part is linked as Excel links it: workbook `extLst` caches
(`x14:slicerCaches`, `x15:slicerCaches`, `x15:timelineCacheRefs`), sheet
`extLst` lists, `#N/A` defined names, and drawing anchors in
`mc:AlternateContent` with Excel's fallback shape.

    python3 scripts/corpus/gen_slicers_xlsx.py           # write the fixture
    python3 scripts/corpus/gen_slicers_xlsx.py --check   # CI: it is up to date
"""

import argparse
import sys
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import gen_pivot_xlsx as g  # noqa: E402

FIXTURE = g.ROOT / "tests/corpus/xlsx/25_slicers.xlsx"
X15 = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main"
REL11 = "http://schemas.microsoft.com/office/2011/relationships"
SLE = "http://schemas.microsoft.com/office/drawing/2010/slicer"
SLE15 = "http://schemas.microsoft.com/office/drawing/2012/slicer"
TSLE = "http://schemas.microsoft.com/office/drawing/2012/timeslicer"
A14 = "http://schemas.microsoft.com/office/drawing/2010/main"
A15 = "http://schemas.microsoft.com/office/drawing/2012/main"
IGN = f'xmlns:mc="{g.MC}" mc:Ignorable="x" xmlns:x="{g.MAIN}"'


def anchor(frm: tuple[int, int], to: tuple[int, int], frame: str, requires: str, ns: str, cid: int) -> str:
    """A two-cell anchor holding `frame` (an object Excel 2010+ draws) with
    Excel's fallback shape."""
    fallback = (
        f'<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{cid + 100}" name=""/>'
        '<xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr>'
        '<xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="1524000"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr>'
        '<xdr:txBody><a:bodyPr vertOverflow="clip" horzOverflow="clip"/><a:lstStyle/>'
        '<a:p><a:r><a:rPr lang="en-US" sz="1100"/><a:t>This shape represents a slicer. '
        "Slicers are supported in Excel 2010 or later.</a:t></a:r></a:p></xdr:txBody></xdr:sp>"
    )
    marker = lambda tag, c, r: (  # noqa: E731
        f"<xdr:{tag}><xdr:col>{c}</xdr:col><xdr:colOff>0</xdr:colOff>"
        f"<xdr:row>{r}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:{tag}>"
    )
    return (
        '<xdr:twoCellAnchor editAs="oneCell">'
        + marker("from", *frm)
        + marker("to", *to)
        + f'<mc:AlternateContent xmlns:mc="{g.MC}">'
        f'<mc:Choice xmlns:{requires}="{ns}" Requires="{requires}">{frame}</mc:Choice>'
        f"<mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"
        "<xdr:clientData/></xdr:twoCellAnchor>"
    )


def graphic(cid: int, name: str, uri: str, element: str) -> str:
    return (
        f'<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{cid}" name="{name}"/>'
        "<xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>"
        '<xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>'
        f'<a:graphic><a:graphicData uri="{uri}">{element}</a:graphicData></a:graphic>'
        "</xdr:graphicFrame>"
    )


def drawing(anchors: list[str]) -> str:
    return f'{g.HEAD}<xdr:wsDr xmlns:xdr="{g.XDR}" xmlns:a="{g.DML}">{"".join(anchors)}</xdr:wsDr>'


def build() -> dict[str, str]:
    parts: dict[str, str] = {}
    sst: dict[str, int] = {}
    cache = g.Cache(1, 5, {"ref": "A1:E13", "sheet": "Data"}, 1)
    pivot = g.Pivot("PivotTable1", "Report", 3, 1, cache, rows=[1], cols=[], data=[(4, "sum")])

    # Data: the table and its slicer.
    data_cells: dict = {}
    for c, h in enumerate(g.HEADERS, start=1):
        data_cells[(1, c)] = h
    for r, row in enumerate(g.ROWS, start=2):
        for c, v in enumerate(row, start=1):
            data_cells[(r, c)] = (g.serial(v), 1) if c == 3 else v
    parts["xl/worksheets/sheet1.xml"] = g.sheet_xml(
        data_cells,
        sst,
        '<drawing r:id="rId2"/><tableParts count="1"><tablePart r:id="rId1"/></tableParts>'
        f'<extLst><ext uri="{{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}}" xmlns:x15="{X15}">'
        '<x14:slicerList xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">'
        '<x14:slicer r:id="rId3"/></x14:slicerList></ext></extLst>',
        {3: 11},
    )
    parts["xl/worksheets/_rels/sheet1.xml.rels"] = g.rels(
        [
            ("rId1", "table", "../tables/table1.xml"),
            ("rId2", "drawing", "../drawings/drawing1.xml"),
            ("rId3", f"{g.MS_REL}/slicer", "../slicers/slicer1.xml"),
        ]
    )
    cols = "".join(f'<tableColumn id="{i}" name="{h}"/>' for i, h in enumerate(g.HEADERS, start=1))
    parts["xl/tables/table1.xml"] = (
        f'{g.HEAD}<table xmlns="{g.MAIN}" id="1" name="Sales" displayName="Sales" ref="A1:E13">'
        '<autoFilter ref="A1:E13"/>'
        f'<tableColumns count="{len(g.HEADERS)}">{cols}</tableColumns>'
        '<tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" '
        'showRowStripes="1" showColumnStripes="0"/></table>'
    )
    parts["xl/slicers/slicer1.xml"] = (
        f'{g.HEAD}<slicers xmlns="{g.X14}" {IGN}>'
        '<slicer name="Product" cache="Slicer_Product" caption="Product" rowHeight="241300"/></slicers>'
    )
    parts["xl/slicerCaches/slicerCache1.xml"] = (
        f'{g.HEAD}<slicerCacheDefinition xmlns="{g.X14}" {IGN} name="Slicer_Product" sourceName="Product">'
        f'<extLst><x:ext uri="{{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}}" xmlns:x15="{X15}">'
        '<x15:tableSlicerCache tableId="1" column="2"/></x:ext></extLst></slicerCacheDefinition>'
    )
    parts["xl/drawings/drawing1.xml"] = drawing(
        [anchor((6, 1), (9, 12), graphic(2, "Product", SLE, f'<sle:slicer xmlns:sle="{SLE}" name="Product"/>'),
                "sle15", SLE15, 2)]
    )

    # Report: the pivot table, a slicer on Region and a timeline on Date.
    cells, loc = g.render(pivot)
    parts["xl/pivotTables/pivotTable1.xml"] = g.pivot_xml(pivot, loc)
    parts["xl/pivotTables/_rels/pivotTable1.xml.rels"] = g.rels(
        [("rId1", "pivotCacheDefinition", "../pivotCache/pivotCacheDefinition1.xml")]
    )
    parts["xl/worksheets/sheet2.xml"] = g.sheet_xml(
        cells,
        sst,
        '<drawing r:id="rId2"/><extLst>'
        f'<ext uri="{{A8765BA9-456A-4dab-B4F3-ACF838C121DE}}" xmlns:x14="{g.X14}">'
        '<x14:slicerList><x14:slicer r:id="rId3"/></x14:slicerList></ext>'
        f'<ext uri="{{7E03D99C-DC04-49d9-9315-930204A7B6E9}}" xmlns:x15="{X15}">'
        '<x15:timelineRefs><x15:timelineRef r:id="rId4"/></x15:timelineRefs></ext></extLst>',
        {1: 14, 2: 16},
    )
    parts["xl/worksheets/_rels/sheet2.xml.rels"] = g.rels(
        [
            ("rId1", "pivotTable", "../pivotTables/pivotTable1.xml"),
            ("rId2", "drawing", "../drawings/drawing2.xml"),
            ("rId3", f"{g.MS_REL}/slicer", "../slicers/slicer2.xml"),
            ("rId4", f"{REL11}/timeline", "../timelines/timeline1.xml"),
        ]
    )
    items = "".join(f'<i x="{g.shared(0).index(v)}" s="1"/>' for v in sorted(g.shared(0)))
    parts["xl/slicerCaches/slicerCache2.xml"] = (
        f'{g.HEAD}<slicerCacheDefinition xmlns="{g.X14}" {IGN} name="Slicer_Region" sourceName="Region">'
        '<pivotTables><pivotTable tabId="2" name="PivotTable1"/></pivotTables>'
        f'<data><tabular pivotCacheId="1"><items count="{len(g.shared(0))}">{items}</items></tabular></data>'
        "</slicerCacheDefinition>"
    )
    parts["xl/slicers/slicer2.xml"] = (
        f'{g.HEAD}<slicers xmlns="{g.X14}" {IGN}>'
        '<slicer name="Region" cache="Slicer_Region" caption="Region" rowHeight="241300"/></slicers>'
    )
    parts["xl/timelineCaches/timelineCache1.xml"] = (
        f'{g.HEAD}<timelineCacheDefinition xmlns="{X15}" {IGN} name="NativeTimeline_Date" sourceName="Date">'
        '<pivotTables><pivotTable tabId="2" name="PivotTable1"/></pivotTables>'
        '<state minimalRefreshVersion="6" lastRefreshVersion="6" pivotCacheId="1" filterType="unknown">'
        '<bounds startDate="2026-01-01T00:00:00" endDate="2027-01-01T00:00:00"/></state>'
        "</timelineCacheDefinition>"
    )
    parts["xl/timelines/timeline1.xml"] = (
        f'{g.HEAD}<timelines xmlns="{X15}" {IGN}>'
        '<timeline name="NativeTimeline_Date" cache="NativeTimeline_Date" caption="Date" '
        'level="2" selectionLevel="2" scrollPosition="2026-01-01T00:00:00"/></timelines>'
    )
    parts["xl/drawings/drawing2.xml"] = drawing(
        [
            anchor((4, 1), (6, 10), graphic(2, "Region", SLE, f'<sle:slicer xmlns:sle="{SLE}" name="Region"/>'),
                   "a14", A14, 2),
            anchor((4, 11), (11, 18),
                   graphic(3, "Date", TSLE, f'<tsle:timeslicer xmlns:tsle="{TSLE}" name="NativeTimeline_Date"/>'),
                   "a15", A15, 3),
        ]
    )

    d, r = g.cache_xml(cache)
    parts["xl/pivotCache/pivotCacheDefinition1.xml"] = d
    parts["xl/pivotCache/pivotCacheRecords1.xml"] = r
    parts["xl/pivotCache/_rels/pivotCacheDefinition1.xml.rels"] = g.rels(
        [("rId1", "pivotCacheRecords", "pivotCacheRecords1.xml")]
    )

    strings = "".join(f'<si><t xml:space="preserve">{g.escape(s)}</t></si>' for s in sst)
    parts["xl/sharedStrings.xml"] = (
        f'{g.HEAD}<sst xmlns="{g.MAIN}" count="{len(sst)}" uniqueCount="{len(sst)}">{strings}</sst>'
    )
    parts["xl/styles.xml"] = (
        f'{g.HEAD}<styleSheet xmlns="{g.MAIN}">'
        '<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
        '<fills count="2"><fill><patternFill patternType="none"/></fill>'
        '<fill><patternFill patternType="gray125"/></fill></fills>'
        '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
        '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        '<cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
        '<xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs>'
        '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>'
        '<dxfs count="0"/><tableStyles count="0" defaultTableStyle="TableStyleMedium2" '
        'defaultPivotStyle="PivotStyleLight16"/>'
        "</styleSheet>"
    )
    parts["xl/workbook.xml"] = (
        f'{g.HEAD}<workbook {g.NS}>'
        '<bookViews><workbookView activeTab="1"/></bookViews>'
        '<sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Report" sheetId="2" r:id="rId2"/></sheets>'
        '<definedNames><definedName name="NativeTimeline_Date">#N/A</definedName>'
        '<definedName name="Slicer_Product">#N/A</definedName>'
        '<definedName name="Slicer_Region">#N/A</definedName></definedNames>'
        '<calcPr calcId="191029"/>'
        '<pivotCaches><pivotCache cacheId="1" r:id="rId5"/></pivotCaches>'
        "<extLst>"
        f'<ext uri="{{BBE1A952-AA13-448e-AADC-164F8A28A991}}" xmlns:x14="{g.X14}">'
        '<x14:slicerCaches><x14:slicerCache r:id="rId7"/></x14:slicerCaches></ext>'
        f'<ext uri="{{46BE6895-7355-4a93-B00E-2C351335B9C9}}" xmlns:x15="{X15}">'
        f'<x15:slicerCaches xmlns:x14="{g.X14}"><x14:slicerCache r:id="rId6"/></x15:slicerCaches></ext>'
        f'<ext uri="{{D0CA8CA8-9F24-4464-BF8E-62219DCF47F9}}" xmlns:x15="{X15}">'
        '<x15:timelineCacheRefs><x15:timelineCacheRef r:id="rId8"/></x15:timelineCacheRefs></ext>'
        "</extLst></workbook>"
    )
    parts["xl/_rels/workbook.xml.rels"] = g.rels(
        [
            ("rId1", "worksheet", "worksheets/sheet1.xml"),
            ("rId2", "worksheet", "worksheets/sheet2.xml"),
            ("rId3", "styles", "styles.xml"),
            ("rId4", "sharedStrings", "sharedStrings.xml"),
            ("rId5", "pivotCacheDefinition", "pivotCache/pivotCacheDefinition1.xml"),
            ("rId6", f"{g.MS_REL}/slicerCache", "slicerCaches/slicerCache1.xml"),
            ("rId7", f"{g.MS_REL}/slicerCache", "slicerCaches/slicerCache2.xml"),
            ("rId8", f"{REL11}/timelineCache", "timelineCaches/timelineCache1.xml"),
        ]
    )
    parts["_rels/.rels"] = g.rels([("rId1", "officeDocument", "xl/workbook.xml")])
    over = [("/xl/workbook.xml", f"{g.CT}.sheet.main+xml")]
    over += [(f"/xl/worksheets/sheet{i}.xml", f"{g.CT}.worksheet+xml") for i in (1, 2)]
    over += [("/xl/styles.xml", f"{g.CT}.styles+xml"), ("/xl/sharedStrings.xml", f"{g.CT}.sharedStrings+xml")]
    over += [("/xl/tables/table1.xml", f"{g.CT}.table+xml")]
    over += [(f"/xl/slicers/slicer{i}.xml", "application/vnd.ms-excel.slicer+xml") for i in (1, 2)]
    over += [(f"/xl/slicerCaches/slicerCache{i}.xml", "application/vnd.ms-excel.slicerCache+xml") for i in (1, 2)]
    over += [("/xl/timelines/timeline1.xml", "application/vnd.ms-excel.timeline+xml")]
    over += [("/xl/timelineCaches/timelineCache1.xml", "application/vnd.ms-excel.timelineCache+xml")]
    over += [(f"/xl/drawings/drawing{i}.xml", "application/vnd.openxmlformats-officedocument.drawing+xml") for i in (1, 2)]
    over += [("/xl/pivotTables/pivotTable1.xml", f"{g.CT}.pivotTable+xml")]
    over += [
        ("/xl/pivotCache/pivotCacheDefinition1.xml", f"{g.CT}.pivotCacheDefinition+xml"),
        ("/xl/pivotCache/pivotCacheRecords1.xml", f"{g.CT}.pivotCacheRecords+xml"),
    ]
    parts["[Content_Types].xml"] = (
        f'{g.HEAD}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        + "".join(f'<Override PartName="{n}" ContentType="{t}"/>' for n, t in over)
        + "</Types>"
    )
    return parts


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    if args.check:
        tmp = FIXTURE.with_suffix(".check.xlsx")
        g.write(tmp, build())
        same = FIXTURE.exists() and tmp.read_bytes() == FIXTURE.read_bytes()
        tmp.unlink()
        if not same:
            print(f"{FIXTURE.relative_to(g.ROOT)} is out of date; run {Path(__file__).name}")
            sys.exit(1)
        return
    g.write(FIXTURE, build())


if __name__ == "__main__":
    main()
