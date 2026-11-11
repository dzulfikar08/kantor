#!/usr/bin/env python3
"""Write the pivot table fixture (CC0, synthetic, deterministic):
tests/corpus/xlsx/21_pivot_tables.xlsx.

XlsxWriter can't write pivot tables, so this writes raw SpreadsheetML with the
Python standard library: a data sheet that is also an Excel table, two pivot
caches (one over a sheet range, one over the table's name) with their
records, and three pivot tables:

* `Pivot!A3`: compact, rows Region, columns Product (in a hand-made order:
  Plums, Apples, Pears), Sum of Revenue, PivotStyleLight16;
* `Pivot!H3`: compact, a report filter (Product) above it, rows Region with
  West hidden, two data fields (Sum of Units, Average of Revenue) in columns;
* `Tabular!A1`: tabular, over the table `Sales`, rows Region then Product
  with Region subtotals at the bottom, Sum of Units, PivotStyleMedium9.

The cells under each pivot are worked out here the way Excel lays them out,
so tests can compare Kantor's refresh with them.

    python3 scripts/corpus/gen_pivot_xlsx.py           # write the fixture
    python3 scripts/corpus/gen_pivot_xlsx.py --check   # CI: it is up to date
"""

import argparse
import datetime as dt
import statistics
import sys
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from xml.sax.saxutils import escape, quoteattr

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/corpus/xlsx/21_pivot_tables.xlsx"
ZIP_TIME = (2026, 1, 1, 0, 0, 0)

MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS = f'xmlns="{MAIN}" xmlns:r="{REL}"'
PKG_RELS = 'xmlns="http://schemas.openxmlformats.org/package/2006/relationships"'
CT = "application/vnd.openxmlformats-officedocument.spreadsheetml"
HEAD = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'

HEADERS = ["Region", "Product", "Date", "Units", "Revenue"]
ROWS = [
    ("East", "Apples", "2026-01-05", 10, 120),
    ("West", "Pears", "2026-01-09", 4, 60),
    ("North", "Apples", "2026-02-02", 7, 84),
    ("East", "Pears", "2026-02-14", 3, 45),
    ("West", "Apples", "2026-02-20", 12, 144),
    ("North", "Plums", "2026-03-01", 5, 90),
    ("East", "Plums", "2026-03-03", 8, 144),
    ("West", "Plums", "2026-03-15", 2, 36),
    ("North", "Pears", "2026-03-21", 6, 90),
    ("East", "Apples", "2026-04-02", 9, 108),
    ("West", "Pears", "2026-04-11", 1, 15),
    ("North", "Apples", "2026-04-30", 11, 132),
]
SERIAL_EPOCH = dt.date(1899, 12, 30)


def serial(iso: str) -> int:
    return (dt.date.fromisoformat(iso) - SERIAL_EPOCH).days


def col_name(c: int) -> str:
    s = ""
    while c:
        c, r = divmod(c - 1, 26)
        s = chr(65 + r) + s
    return s


def a1(r: int, c: int) -> str:
    return f"{col_name(c)}{r}"


def num(v) -> str:
    if isinstance(v, float) and v.is_integer():
        v = int(v)
    return repr(v) if isinstance(v, float) else str(v)


# ------------------------------------------------------------------ pivots

FUNCS = {
    "sum": ("Sum", sum),
    "count": ("Count", len),
    "average": ("Average", statistics.fmean),
    "max": ("Max", max),
    "min": ("Min", min),
}


@dataclass
class Cache:
    cache_id: int
    rid: int  # workbook relationship number
    source: dict  # worksheetSource attributes
    part: int  # pivotCacheDefinition{part}.xml


@dataclass
class Pivot:
    name: str
    sheet: str
    top: int  # location, 1-based
    left: int
    cache: Cache
    rows: list[int]
    cols: list[int]
    data: list[tuple[int, str]]  # (field, function)
    pages: list[int] = field(default_factory=list)
    hidden: dict[int, set[str]] = field(default_factory=dict)
    order: dict[int, list[str]] = field(default_factory=dict)
    tabular: bool = False
    style: str = "PivotStyleLight16"


def shared(fld: int) -> list[str]:
    """A string field's items in the order they first appear (the cache's)."""
    out: list[str] = []
    for row in ROWS:
        if row[fld] not in out:
            out.append(row[fld])
    return out


def display(p: Pivot, fld: int) -> list[str]:
    return p.order.get(fld) or sorted(shared(fld))


def shown(p: Pivot, fld: int) -> list[str]:
    return [v for v in display(p, fld) if v not in p.hidden.get(fld, set())]


def caption(fld: int, func: str) -> str:
    return f"{FUNCS[func][0]} of {HEADERS[fld]}"


def records(p: Pivot) -> list[tuple]:
    """Source rows the pivot's filters let through."""
    return [
        r for r in ROWS if all(r[f] not in hs for f, hs in p.hidden.items())
    ]


def agg(rows, fld: int, func: str):
    vals = [r[fld] for r in rows]
    return FUNCS[func][1](vals) if vals else None


def row_axis(p: Pivot) -> list[tuple[str, int, tuple]]:
    """(kind, r, path) for each row item: kind '' (data), 'default'
    (subtotal) or 'grand'; path are display indices of the items."""
    out = []

    def walk(depth: int, path: tuple):
        if depth == len(p.rows):
            return
        fld = p.rows[depth]
        for v in shown(p, fld):
            idx = display(p, fld).index(v)
            here = path + (idx,)
            leaf = depth == len(p.rows) - 1
            if leaf or not p.tabular:
                out.append(("", depth, here))
            walk(depth + 1, here)
            if not leaf and p.tabular:
                out.append(("default", depth, here))

    walk(0, ())
    # In tabular form the first leaf of a group carries its parents' labels.
    if p.tabular:
        fixed = []
        prev: tuple = ()
        for kind, r, path in out:
            if kind == "":
                common = 0
                while common < len(path) - 1 and common < len(prev) and prev[common] == path[common]:
                    common += 1
                r = common
                prev = path
            else:
                prev = ()
            fixed.append((kind, r, path))
        out = fixed
    out.append(("grand", 0, (0,)))
    return out


def col_axis(p: Pivot) -> list[tuple[str, int, tuple]]:
    if len(p.data) > 1:
        return [("", 0, (i,)) for i in range(len(p.data))]
    if not p.cols:
        return [("", 0, ())]
    fld = p.cols[0]
    out = [("", 0, (display(p, fld).index(v),)) for v in shown(p, fld)]
    return out + [("grand", 0, (0,))]


def match(p: Pivot, rows, flds, path) -> list:
    out = rows
    for f, idx in zip(flds, path):
        v = display(p, f)[idx]
        out = [r for r in out if r[f] == v]
    return out


def render(p: Pivot) -> tuple[dict, dict]:
    """The pivot's cells by (row, col) and its location attributes."""
    cells: dict[tuple[int, int], object] = {}
    rows_ax = row_axis(p)
    cols_ax = col_axis(p)
    label_cols = len(p.rows) if p.tabular else 1
    two_header_rows = bool(p.cols)
    data_row0 = 2 if two_header_rows else 1
    t, l = p.top, p.left
    # Page fields, two rows above.
    for i, f in enumerate(p.pages):
        r = t - 1 - len(p.pages) + i
        cells[(r, l)] = HEADERS[f]
        cells[(r, l + 1)] = "(All)"
    # Headers.
    if two_header_rows:
        cells[(t, l)] = caption(*p.data[0])
        cells[(t, l + label_cols)] = "Column Labels"
    hr = t + data_row0 - 1
    if p.tabular:
        for i, f in enumerate(p.rows):
            cells[(hr, l + i)] = HEADERS[f]
    else:
        cells[(hr, l)] = "Row Labels"
    for j, (kind, _, path) in enumerate(cols_ax):
        c = l + label_cols + j
        if len(p.data) > 1:
            cells[(hr, c)] = caption(*p.data[path[0]])
        elif not p.cols:
            cells[(hr, c)] = caption(*p.data[0])
        elif kind == "grand":
            cells[(hr, c)] = "Grand Total"
        else:
            cells[(hr, c)] = display(p, p.cols[0])[path[0]]
    # Body.
    src = records(p)
    for i, (kind, r, path) in enumerate(rows_ax):
        row = t + data_row0 + i
        if kind == "grand":
            cells[(row, l)] = "Grand Total"
            sel = src
        else:
            sel = match(p, src, p.rows, path)
            depth = len(path) - 1
            v = display(p, p.rows[depth])[path[depth]]
            if kind == "default":
                cells[(row, l + depth)] = f"{v} Total"
            elif p.tabular:
                for d in range(r, len(path)):
                    cells[(row, l + d)] = display(p, p.rows[d])[path[d]]
            else:
                cells[(row, l)] = v
        for j, (ckind, _, cpath) in enumerate(cols_ax):
            if len(p.data) > 1:
                f, func = p.data[cpath[0]]
                cs = sel
            else:
                f, func = p.data[0]
                cs = sel if ckind == "grand" or not p.cols else match(p, sel, p.cols, cpath)
            v = agg(cs, f, func)
            if v is not None:
                cells[(row, l + label_cols + j)] = v
    bottom = t + data_row0 + len(rows_ax) - 1
    right = l + label_cols + len(cols_ax) - 1
    loc = {
        "ref": f"{a1(t, l)}:{a1(bottom, right)}",
        "firstHeaderRow": "1" if (p.cols or len(p.data) == 1) else "0",
        "firstDataRow": str(data_row0),
        "firstDataCol": str(label_cols),
    }
    if p.pages:
        loc["rowPageCount"] = str(len(p.pages))
        loc["colPageCount"] = "1"
    return cells, loc


def items_xml(items: list[tuple[str, int, tuple]], first: int = 0) -> str:
    out = ""
    for kind, r, path in items:
        attrs = ""
        if r:
            attrs += f' r="{r}"'
        if kind:
            attrs += f' t="{kind}"'
        xs = "".join("<x/>" if v == 0 else f'<x v="{v}"/>' for v in path[r:])
        out += f"<i{attrs}>{xs}</i>"
    return out


def pivot_xml(p: Pivot, loc: dict) -> str:
    flds = ""
    for f, name in enumerate(HEADERS):
        a = ""
        if p.tabular:
            a += ' compact="0" outline="0"'
        if f in p.rows:
            a += ' axis="axisRow"'
        elif f in p.cols:
            a += ' axis="axisCol"'
        elif f in p.pages:
            a += ' axis="axisPage"'
        if any(d == f for d, _ in p.data):
            a += ' dataField="1"'
        if f == 2:
            a += ' numFmtId="14"'
        a += ' showAll="0"'
        if f in p.rows or f in p.cols or f in p.pages:
            cache_items = shared(f)
            its = ""
            for v in display(p, f):
                h = ' h="1"' if v in p.hidden.get(f, set()) else ""
                its += f'<item{h} x="{cache_items.index(v)}"/>'
            n = len(cache_items) + 1
            flds += f'<pivotField{a}><items count="{n}">{its}<item t="default"/></items></pivotField>'
        else:
            flds += f"<pivotField{a}/>"
    rows_ax = row_axis(p)
    cols_ax = col_axis(p)
    body = f'<location {" ".join(f"{k}={quoteattr(v)}" for k, v in loc.items())}/>'
    body += f'<pivotFields count="{len(HEADERS)}">{flds}</pivotFields>'
    body += f'<rowFields count="{len(p.rows)}">' + "".join(f'<field x="{f}"/>' for f in p.rows) + "</rowFields>"
    body += f'<rowItems count="{len(rows_ax)}">{items_xml(rows_ax)}</rowItems>'
    col_fields = p.cols + ([-2] if len(p.data) > 1 else [])
    if col_fields:
        body += f'<colFields count="{len(col_fields)}">' + "".join(f'<field x="{f}"/>' for f in col_fields) + "</colFields>"
    if len(p.data) > 1:
        col_items = "".join(
            "<i><x/></i>" if i == 0 else f'<i i="{i}"><x v="{i}"/></i>' for i in range(len(p.data))
        )
    elif not p.cols:
        col_items = "<i/>"
    else:
        col_items = items_xml(cols_ax)
    body += f'<colItems count="{len(cols_ax)}">{col_items}</colItems>'
    if p.pages:
        body += f'<pageFields count="{len(p.pages)}">' + "".join(
            f'<pageField fld="{f}" hier="-1"/>' for f in p.pages
        ) + "</pageFields>"
    body += f'<dataFields count="{len(p.data)}">'
    for f, func in p.data:
        sub = "" if func == "sum" else f' subtotal="{func}"'
        body += f'<dataField name={quoteattr(caption(f, func))} fld="{f}"{sub} baseField="0" baseItem="0"/>'
    body += "</dataFields>"
    body += (
        f'<pivotTableStyleInfo name="{p.style}" showRowHeaders="1" showColHeaders="1" '
        'showRowStripes="0" showColStripes="0" showLastColumn="1"/>'
    )
    layout = ' compact="0" compactData="0"' if p.tabular else ' outline="1" outlineData="1"'
    return (
        f'{HEAD}<pivotTableDefinition xmlns="{MAIN}" name={quoteattr(p.name)} '
        f'cacheId="{p.cache.cache_id}" applyNumberFormats="0" applyBorderFormats="0" '
        'applyFontFormats="0" applyPatternFormats="0" applyAlignmentFormats="0" '
        'applyWidthHeightFormats="1" dataCaption="Values" updatedVersion="6" '
        'minRefreshableVersion="3" useAutoFormatting="1" itemPrintTitles="1" '
        f'createdVersion="6" indent="0"{layout} multipleFieldFilters="0">'
        f"{body}</pivotTableDefinition>"
    )


def cache_xml(c: Cache) -> tuple[str, str]:
    fields = ""
    for f, name in enumerate(HEADERS):
        vals = [r[f] for r in ROWS]
        if f in (0, 1):
            its = "".join(f"<s v={quoteattr(v)}/>" for v in shared(f))
            fields += f'<cacheField name="{name}" numFmtId="0"><sharedItems count="{len(shared(f))}">{its}</sharedItems></cacheField>'
        elif f == 2:
            lo, hi = min(vals), max(vals)
            fields += (
                f'<cacheField name="{name}" numFmtId="14"><sharedItems containsSemiMixedTypes="0" '
                f'containsNonDate="0" containsDate="1" containsString="0" '
                f'minDate="{lo}T00:00:00" maxDate="{hi}T00:00:00"/></cacheField>'
            )
        else:
            fields += (
                f'<cacheField name="{name}" numFmtId="0"><sharedItems containsSemiMixedTypes="0" '
                f'containsString="0" containsNumber="1" containsInteger="1" '
                f'minValue="{min(vals)}" maxValue="{max(vals)}"/></cacheField>'
            )
    src = " ".join(f"{k}={quoteattr(v)}" for k, v in c.source.items())
    definition = (
        f'{HEAD}<pivotCacheDefinition {NS} r:id="rId1" refreshedBy="Kantor corpus" '
        'refreshedDate="46023.5" createdVersion="6" refreshedVersion="6" '
        f'minRefreshableVersion="3" recordCount="{len(ROWS)}">'
        f'<cacheSource type="worksheet"><worksheetSource {src}/></cacheSource>'
        f'<cacheFields count="{len(HEADERS)}">{fields}</cacheFields></pivotCacheDefinition>'
    )
    recs = ""
    for r in ROWS:
        recs += (
            f'<r><x v="{shared(0).index(r[0])}"/><x v="{shared(1).index(r[1])}"/>'
            f'<d v="{r[2]}T00:00:00"/><n v="{r[3]}"/><n v="{r[4]}"/></r>'
        )
    records_ = (
        f'{HEAD}<pivotCacheRecords {NS} count="{len(ROWS)}">{recs}</pivotCacheRecords>'
    )
    return definition, records_


# ------------------------------------------------------------------ package


def sheet_xml(cells: dict, sst: dict, extra: str = "", widths: dict | None = None) -> str:
    rows: dict[int, list[str]] = {}
    for (r, c), v in sorted(cells.items()):
        ref = a1(r, c)
        if isinstance(v, str):
            idx = sst.setdefault(v, len(sst))
            cell = f'<c r="{ref}" t="s"><v>{idx}</v></c>'
        elif isinstance(v, tuple):  # (serial, style)
            cell = f'<c r="{ref}" s="{v[1]}"><v>{v[0]}</v></c>'
        else:
            cell = f'<c r="{ref}"><v>{num(v)}</v></c>'
        rows.setdefault(r, []).append(cell)
    data = "".join(f'<row r="{r}">{"".join(cs)}</row>' for r, cs in sorted(rows.items()))
    cols = ""
    if widths:
        cols = "<cols>" + "".join(
            f'<col min="{c}" max="{c}" width="{w}" customWidth="1"/>' for c, w in sorted(widths.items())
        ) + "</cols>"
    return (
        f"{HEAD}<worksheet {NS}>{cols}<sheetData>{data}</sheetData>"
        '<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>'
        f"{extra}</worksheet>"
    )


def rels(items: list[tuple[str, str, str]]) -> str:
    body = "".join(
        f'<Relationship Id="{i}" Type="{REL}/{t}" Target="{target}"/>' for i, t, target in items
    )
    return f"{HEAD}<Relationships {PKG_RELS}>{body}</Relationships>"


def build() -> dict[str, str]:
    cache1 = Cache(1, 5, {"ref": "A1:E13", "sheet": "Data"}, 1)
    cache2 = Cache(2, 6, {"name": "Sales"}, 2)
    pivots = [
        Pivot("PivotTable1", "Pivot", 3, 1, cache1, rows=[0], cols=[1], data=[(4, "sum")],
              order={1: ["Plums", "Apples", "Pears"]}),
        Pivot("PivotTable2", "Pivot", 3, 8, cache1, rows=[0], cols=[], pages=[1],
              data=[(3, "sum"), (4, "average")], hidden={0: {"West"}}),
        Pivot("PivotTable3", "Tabular", 1, 1, cache2, rows=[0, 1], cols=[], data=[(3, "sum")],
              tabular=True, style="PivotStyleMedium9"),
    ]
    sst: dict[str, int] = {}
    parts: dict[str, str] = {}

    data_cells: dict = {}
    for c, h in enumerate(HEADERS, start=1):
        data_cells[(1, c)] = h
    for r, row in enumerate(ROWS, start=2):
        for c, v in enumerate(row, start=1):
            data_cells[(r, c)] = (serial(v), 1) if c == 3 else v
    parts["xl/worksheets/sheet1.xml"] = sheet_xml(
        data_cells, sst, '<tableParts count="1"><tablePart r:id="rId1"/></tableParts>',
        {3: 11},
    )
    parts["xl/worksheets/_rels/sheet1.xml.rels"] = rels([("rId1", "table", "../tables/table1.xml")])
    cols = "".join(f'<tableColumn id="{i}" name="{h}"/>' for i, h in enumerate(HEADERS, start=1))
    parts["xl/tables/table1.xml"] = (
        f'{HEAD}<table xmlns="{MAIN}" id="1" name="Sales" displayName="Sales" ref="A1:E13">'
        '<autoFilter ref="A1:E13"/>'
        f'<tableColumns count="{len(HEADERS)}">{cols}</tableColumns>'
        '<tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" '
        'showRowStripes="1" showColumnStripes="0"/></table>'
    )

    by_sheet: dict[str, list[Pivot]] = {}
    for p in pivots:
        by_sheet.setdefault(p.sheet, []).append(p)
    sheet_names = ["Data", "Pivot", "Tabular"]
    for n, name in enumerate(sheet_names[1:], start=2):
        cells: dict = {}
        prels = []
        for p in by_sheet[name]:
            pc, loc = render(p)
            cells.update(pc)
            k = pivots.index(p) + 1
            parts[f"xl/pivotTables/pivotTable{k}.xml"] = pivot_xml(p, loc)
            parts[f"xl/pivotTables/_rels/pivotTable{k}.xml.rels"] = rels(
                [("rId1", "pivotCacheDefinition", f"../pivotCache/pivotCacheDefinition{p.cache.part}.xml")]
            )
            prels.append((f"rId{len(prels) + 1}", "pivotTable", f"../pivotTables/pivotTable{k}.xml"))
        widths = {1: 14, 2: 16, 3: 16, 4: 10, 5: 12, 8: 14, 9: 14, 10: 20}
        parts[f"xl/worksheets/sheet{n}.xml"] = sheet_xml(cells, sst, widths=widths)
        parts[f"xl/worksheets/_rels/sheet{n}.xml.rels"] = rels(prels)
    for c in (cache1, cache2):
        d, r = cache_xml(c)
        parts[f"xl/pivotCache/pivotCacheDefinition{c.part}.xml"] = d
        parts[f"xl/pivotCache/pivotCacheRecords{c.part}.xml"] = r
        parts[f"xl/pivotCache/_rels/pivotCacheDefinition{c.part}.xml.rels"] = rels(
            [("rId1", "pivotCacheRecords", f"pivotCacheRecords{c.part}.xml")]
        )

    strings = "".join(f'<si><t xml:space="preserve">{escape(s)}</t></si>' for s in sst)
    parts["xl/sharedStrings.xml"] = (
        f'{HEAD}<sst xmlns="{MAIN}" count="{len(sst)}" uniqueCount="{len(sst)}">{strings}</sst>'
    )
    parts["xl/styles.xml"] = (
        f'{HEAD}<styleSheet xmlns="{MAIN}">'
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
    sheet_list = "".join(
        f'<sheet name="{n}" sheetId="{i}" r:id="rId{i}"/>' for i, n in enumerate(sheet_names, start=1)
    )
    parts["xl/workbook.xml"] = (
        f"{HEAD}<workbook {NS}><workbookPr defaultThemeVersion=\"166925\"/>"
        '<bookViews><workbookView activeTab="1"/></bookViews>'
        f"<sheets>{sheet_list}</sheets>"
        '<definedNames><definedName name="SalesData">Data!$A$1:$E$13</definedName></definedNames>'
        '<calcPr calcId="191029"/>'
        f'<pivotCaches><pivotCache cacheId="{cache1.cache_id}" r:id="rId{cache1.rid}"/>'
        f'<pivotCache cacheId="{cache2.cache_id}" r:id="rId{cache2.rid}"/></pivotCaches>'
        "</workbook>"
    )
    wb_rels = [(f"rId{i}", "worksheet", f"worksheets/sheet{i}.xml") for i in (1, 2, 3)]
    wb_rels += [("rId4", "styles", "styles.xml")]
    wb_rels += [(f"rId{c.rid}", "pivotCacheDefinition", f"pivotCache/pivotCacheDefinition{c.part}.xml") for c in (cache1, cache2)]
    wb_rels += [("rId7", "sharedStrings", "sharedStrings.xml")]
    parts["xl/_rels/workbook.xml.rels"] = rels(wb_rels)
    parts["_rels/.rels"] = rels([("rId1", "officeDocument", "xl/workbook.xml")])

    over = [("/xl/workbook.xml", f"{CT}.sheet.main+xml")]
    over += [(f"/xl/worksheets/sheet{i}.xml", f"{CT}.worksheet+xml") for i in (1, 2, 3)]
    over += [("/xl/styles.xml", f"{CT}.styles+xml"), ("/xl/sharedStrings.xml", f"{CT}.sharedStrings+xml")]
    over += [("/xl/tables/table1.xml", f"{CT}.table+xml")]
    over += [(f"/xl/pivotTables/pivotTable{k}.xml", f"{CT}.pivotTable+xml") for k in (1, 2, 3)]
    for c in (cache1, cache2):
        over += [
            (f"/xl/pivotCache/pivotCacheDefinition{c.part}.xml", f"{CT}.pivotCacheDefinition+xml"),
            (f"/xl/pivotCache/pivotCacheRecords{c.part}.xml", f"{CT}.pivotCacheRecords+xml"),
        ]
    parts["[Content_Types].xml"] = (
        f'{HEAD}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        + "".join(f'<Override PartName="{n}" ContentType="{t}"/>' for n, t in over)
        + "</Types>"
    )
    return parts


def write(path: Path) -> None:
    parts = build()
    order = ["[Content_Types].xml", "_rels/.rels", "xl/workbook.xml", "xl/_rels/workbook.xml.rels"]
    order += sorted(p for p in parts if p not in order)
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name in order:
            info = zipfile.ZipInfo(name, ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, parts[name].encode("utf-8"))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    if args.check:
        tmp = FIXTURE.with_suffix(".check.xlsx")
        write(tmp)
        same = FIXTURE.exists() and tmp.read_bytes() == FIXTURE.read_bytes()
        tmp.unlink()
        if not same:
            print(f"{FIXTURE.relative_to(ROOT)} is out of date; run {Path(__file__).name}")
            sys.exit(1)
        return
    write(FIXTURE)


if __name__ == "__main__":
    main()
